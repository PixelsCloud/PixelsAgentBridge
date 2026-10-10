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

mod device_accounts;
mod devices;
mod events;
mod github;
pub use github::GithubConfig;
mod management;
mod saved_devices;
mod session;
mod support;
mod usage;
mod user_context;
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
    github: Option<Arc<GithubConfig>>,
    origin: Option<String>,
}

pub fn router(control: ControlApiState) -> Router {
    let state = WebState {
        control: control.control,
        registration_enabled: control.config.registration_enabled,
        login_slots: Arc::new(Semaphore::new(4)),
        login_budget: Arc::new(Mutex::new((Instant::now(), 0))),
        subscriptions: Arc::new(Semaphore::new(256)),
        server_instance: control.server_instance,
        github: control.github,
        origin: control.config.web_origin,
    };
    let browser = Router::new()
        .route("/api/web/config", get(session::config))
        .route(
            "/api/web/session",
            get(session::current).post(session::login),
        )
        .route("/api/web/logout", post(session::logout))
        .route("/api/web/register", post(session::register))
        .route("/api/web/github/start", post(github::web_start))
        .route(
            "/api/web/github",
            get(github::web_status).delete(github::web_unlink),
        )
        .route("/api/web/password", post(session::change_password))
        .route("/api/web/devices", get(devices::list))
        .route(
            "/api/web/saved-devices",
            get(saved_devices::web_changes).post(saved_devices::web_mutate),
        )
        .route("/api/web/usage", get(usage::summary))
        .route(
            "/api/web/devices/{id}/association",
            axum::routing::delete(device_accounts::web_unlink),
        )
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
            axum::routing::put(management::update_user_limit),
        )
        .route("/api/web/audit", get(management::audit))
        .route("/api/web/traffic", get(management::traffic))
        .route("/api/web/relays", get(management::relays))
        .route(
            "/api/web/service",
            get(management::service_config),
        )
        .route(
            "/api",
            any(|| async { WebError::new(axum::http::StatusCode::NOT_FOUND, "not_found") }),
        )
        .route(
            "/api/{*unknown}",
            any(|| async { WebError::new(axum::http::StatusCode::NOT_FOUND, "not_found") }),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            support::browser_boundary,
        ))
        .with_state(state.clone());
    let native = Router::new()
        .route(
            "/api/account/saved-devices",
            get(saved_devices::native_changes).post(saved_devices::native_mutate),
        )
        .route(
            "/api/account/saved-devices/import",
            post(saved_devices::native_import),
        )
        .route("/api/account/usage", post(usage::report))
        .route(
            "/api/account/devices/{id}/association-challenge",
            post(device_accounts::challenge),
        )
        .route(
            "/api/account/devices/{id}/association",
            axum::routing::put(device_accounts::associate).delete(device_accounts::native_unlink),
        )
        .route(
            "/api/account/endpoint-context",
            axum::routing::put(user_context::update),
        )
        .route("/api/account/config", get(session::config))
        .route("/api/account/github/start", post(github::native_start))
        .route("/api/account/github/redeem", post(github::redeem))
        .route(
            "/api/account/github",
            get(github::native_status).delete(github::native_unlink),
        )
        .route(
            "/api/account/session",
            get(session::native_current).post(session::native_login),
        )
        .route("/api/account/register", post(session::native_register))
        .route("/api/account/logout", post(session::native_logout))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            support::native_boundary,
        ))
        .with_state(state.clone());
    // OAuth navigation is cross-site by design. These GETs use their own
    // one-time state + Lax cookie boundary, never the native Origin middleware.
    let oauth = Router::new()
        .route("/api/account/github/authorize", get(github::authorize))
        .route("/api/account/github/callback", get(github::callback))
        .with_state(state);
    browser.merge(native).merge(oauth)
}

pub fn assets(
    root: Option<std::path::PathBuf>,
) -> tower_http::services::ServeDir<tower_http::services::ServeFile> {
    let root = root.unwrap_or_else(|| {
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
