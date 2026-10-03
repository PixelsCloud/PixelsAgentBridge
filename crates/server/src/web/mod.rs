//! Browser-only management API. Remote operations and task history stay local.
use std::{sync::Arc, time::Instant};

use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware,
    routing::{any, get, post},
};
use tokio::sync::{Mutex, Semaphore};

use crate::{ControlApiState, ControlPlane};

mod claims;
mod devices;
mod events;
mod management;
mod session;
mod support;
pub use support::WebError;
pub use support::response_headers;

#[derive(Clone)]
struct WebState {
    control: Arc<ControlPlane>,
    registration_enabled: bool,
    login_slots: Arc<Semaphore>,
    login_budget: Arc<Mutex<(Instant, u32)>>,
    subscriptions: Arc<Semaphore>,
    server_instance: uuid::Uuid,
}

pub fn router(control: ControlApiState) -> Router {
    let state = WebState {
        control: control.control,
        registration_enabled: control.config.registration_enabled,
        login_slots: Arc::new(Semaphore::new(4)),
        login_budget: Arc::new(Mutex::new((Instant::now(), 0))),
        subscriptions: Arc::new(Semaphore::new(256)),
        server_instance: control.server_instance,
    };
    Router::new()
        .route("/api/web/config", get(session::config))
        .route(
            "/api/web/session",
            get(session::current).post(session::login),
        )
        .route("/api/web/logout", post(session::logout))
        .route("/api/web/register", post(session::register))
        .route("/api/web/password", post(session::change_password))
        .route("/api/web/devices", get(devices::list))
        .route(
            "/api/web/devices/{id}",
            get(devices::detail).patch(devices::rename),
        )
        .route("/api/web/overview", get(devices::overview))
        .route("/api/web/events", get(events::subscribe))
        .route("/api/web/accounts", get(management::accounts))
        .route(
            "/api/web/accounts/{id}",
            axum::routing::patch(management::update_account),
        )
        .route(
            "/api/web/accounts/{id}/traffic",
            post(management::assign_team),
        )
        .route(
            "/api/web/teams",
            get(management::teams).post(management::create_team),
        )
        .route("/api/web/teams/{id}/members", get(management::members))
        .route("/api/web/teams/{id}/actions", post(management::team_action))
        .route("/api/web/audit", get(management::audit))
        .route("/api/web/traffic", get(management::traffic))
        .route("/api/web/relays", get(management::relays))
        .route("/api/web/service", get(management::service_config))
        .route(
            "/api/web/accounts/{id}/teams",
            get(management::eligible_teams),
        )
        .route("/api/web/claims", get(claims::list).post(claims::begin))
        .route("/api/web/claims/{id}/cancel", post(claims::cancel))
        .route("/api/web/devices/{id}/unbind", post(claims::unbind))
        .route(
            "/api",
            any(|| async { WebError::new(axum::http::StatusCode::NOT_FOUND, "not_found") }),
        )
        .route(
            "/api/{*unknown}",
            any(|| async { WebError::new(axum::http::StatusCode::NOT_FOUND, "not_found") }),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn(support::browser_boundary))
        .with_state(state)
}

pub fn assets() -> tower_http::services::ServeDir<tower_http::services::ServeFile> {
    let root = std::env::var_os("PAB_WEB_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|path| path.parent().map(|p| p.join("web")))
                .unwrap_or_else(|| "web".into())
        });
    tower_http::services::ServeDir::new(&root).fallback(tower_http::services::ServeFile::new(
        root.join("index.html"),
    ))
}

/// Initial server administrators are explicitly provisioned by the deployment operator.
pub async fn bootstrap_admin(control: &ControlPlane, username: &str) -> Result<(), WebError> {
    let id = control.account_id_by_username(username).await?;
    let mut tx = control.store().pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(26035001)")
        .execute(&mut *tx)
        .await?;
    let changed =
        sqlx::query("UPDATE users SET server_admin = true WHERE id = $1 AND status = 'active'")
            .bind(id.as_uuid())
            .execute(&mut *tx)
            .await?;
    if changed.rows_affected() != 1 {
        return Err(WebError::invalid());
    }
    sqlx::query("INSERT INTO web_admin_events (actor_id, action, resource_id) VALUES ($1, 'admin.bootstrap', $1)")
        .bind(id.as_uuid()).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
