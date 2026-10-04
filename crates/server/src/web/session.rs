use crate::{
    auth::{PasswordEngine, PasswordPolicy, normalize_username},
    web::{WebError, WebState},
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

const COOKIE: &str = "__Host-pab_session";
const MAX_AGE: u64 = 12 * 60 * 60;

#[derive(Serialize)]
pub(crate) struct Viewer {
    pub id: Uuid,
    pub username: String,
    pub personal_tenant_id: Uuid,
    pub server_admin: bool,
    #[serde(skip)]
    pub auth_revision: i64,
}

fn token_hash(headers: &HeaderMap) -> Option<String> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    let mut values = cookie
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .filter(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value);
    let value = values.next()?;
    if values.next().is_some() || value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    Some(blake3::hash(value.as_bytes()).to_hex().to_string())
}

pub(crate) async fn viewer(state: &WebState, headers: &HeaderMap) -> Result<Viewer, WebError> {
    let hash = token_hash(headers).ok_or_else(WebError::unauthorized)?;
    let row = sqlx::query("SELECT u.id, u.username, u.server_admin, u.auth_revision, p.tenant_id FROM web_sessions s JOIN users u ON u.id=s.user_id JOIN personal_tenants p ON p.user_id=u.id WHERE s.token_hash=$1 AND s.expires_at>now() AND s.auth_revision=u.auth_revision AND u.status='active'")
        .bind(hash).fetch_optional(state.control.store().pool()).await?.ok_or_else(WebError::unauthorized)?;
    Ok(Viewer {
        id: row.try_get("id")?,
        username: row.try_get("username")?,
        personal_tenant_id: row.try_get("tenant_id")?,
        server_admin: row.try_get("server_admin")?,
        auth_revision: row.try_get("auth_revision")?,
    })
}

pub(crate) async fn config(State(state): State<WebState>) -> Json<Value> {
    Json(
        json!({"registration_enabled":state.registration_enabled,"version":env!("CARGO_PKG_VERSION")}),
    )
}
pub(crate) async fn current(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Viewer>, WebError> {
    Ok(Json(viewer(&state, &headers).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Credentials {
    username: String,
    password: String,
}

async fn login_permit(state: &WebState) -> Result<tokio::sync::SemaphorePermit<'_>, WebError> {
    let limited = || WebError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    let permit = state.login_slots.try_acquire().map_err(|_| limited())?;
    let mut budget = state.login_budget.lock().await;
    if budget.0.elapsed().as_secs() >= 60 {
        *budget = (std::time::Instant::now(), 0);
    }
    if budget.1 >= 60 {
        return Err(limited());
    }
    budget.1 += 1;
    Ok(permit)
}

pub(crate) async fn login(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(input): Json<Credentials>,
) -> Result<Response, WebError> {
    let _permit = login_permit(&state).await?;
    establish(&state, &headers, input).await
}

async fn establish(
    state: &WebState,
    headers: &HeaderMap,
    input: Credentials,
) -> Result<Response, WebError> {
    if input.password.len() > 1024 {
        return Err(WebError::unauthorized());
    }
    let (_, key) = normalize_username(&input.username).map_err(|_| WebError::unauthorized())?;
    // Capture the revision BEFORE password verification, so concurrent password
    // changes cannot turn an old verified password into a new valid session.
    let revision: Option<i64> = sqlx::query_scalar(
        "SELECT auth_revision FROM users WHERE username_key=$1 AND status='active'",
    )
    .bind(key)
    .fetch_optional(state.control.store().pool())
    .await?;
    let account = state
        .control
        .authenticate(&input.username, &input.password)
        .await?;
    let mut random = [0u8; 32];
    OsRng.fill_bytes(&mut random);
    let token: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let hash = blake3::hash(token.as_bytes()).to_hex().to_string();
    let mut tx = state.control.store().pool().begin().await?;
    // Lock the account through creation to serialize password changes and enforce
    // the per-account session cap across simultaneous browser logins.
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(account.id.as_uuid())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM web_sessions WHERE expires_at<=now() OR token_hash=$1")
        .bind(token_hash(headers).unwrap_or_default())
        .execute(&mut *tx)
        .await?;
    let inserted = sqlx::query("INSERT INTO web_sessions (token_hash,user_id,auth_revision,expires_at) SELECT $1,id,auth_revision,now()+interval '12 hours' FROM users WHERE id=$2 AND auth_revision=$3 AND status='active'")
        .bind(&hash).bind(account.id.as_uuid()).bind(revision).execute(&mut *tx).await?;
    if inserted.rows_affected() != 1 {
        return Err(WebError::unauthorized());
    }
    sqlx::query("DELETE FROM web_sessions WHERE token_hash IN (SELECT token_hash FROM web_sessions WHERE user_id=$1 ORDER BY created_at DESC, token_hash OFFSET 20)")
        .bind(account.id.as_uuid()).execute(&mut *tx).await?;
    tx.commit().await?;
    let mut session_headers = HeaderMap::new();
    session_headers.insert(header::COOKIE, format!("{COOKIE}={token}").parse().unwrap());
    let me = viewer(state, &session_headers).await?;
    Ok((
        [(
            header::SET_COOKIE,
            format!(
                "{COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={MAX_AGE}"
            ),
        )],
        Json(me),
    )
        .into_response())
}

pub(crate) async fn register(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(input): Json<Credentials>,
) -> Result<Response, WebError> {
    if !state.registration_enabled {
        return Err(WebError::new(
            StatusCode::FORBIDDEN,
            "registration_disabled",
        ));
    }
    let _permit = login_permit(&state).await?;
    state
        .control
        .register_account(&input.username, &input.password)
        .await?;
    establish(&state, &headers, input).await
}

pub(crate) async fn logout(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    if let Some(hash) = token_hash(&headers) {
        sqlx::query("DELETE FROM web_sessions WHERE token_hash=$1")
            .bind(hash)
            .execute(state.control.store().pool())
            .await?;
    }
    Ok((
        [(
            header::SET_COOKIE,
            format!("{COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0"),
        )],
        StatusCode::NO_CONTENT,
    )
        .into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PasswordChange {
    current_password: String,
    new_password: String,
}
pub(crate) async fn change_password(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(input): Json<PasswordChange>,
) -> Result<Response, WebError> {
    let me = viewer(&state, &headers).await?;
    let _permit = login_permit(&state).await?;
    if input.current_password.len() > 1024 || input.new_password.len() > 1024 {
        return Err(WebError::invalid());
    }
    state
        .control
        .authenticate(&me.username, &input.current_password)
        .await
        .map_err(|error| match error {
            crate::ServiceError::InvalidCredentials => {
                WebError::new(StatusCode::BAD_REQUEST, "incorrect_password")
            }
            error => WebError::from(error),
        })?;
    let hash = tokio::task::spawn_blocking(move || {
        PasswordEngine::new(PasswordPolicy::default()).hash(&input.new_password)
    })
    .await
    .map_err(|_| WebError::new(StatusCode::INTERNAL_SERVER_ERROR, "server_error"))?
    .map_err(|_| WebError::invalid())?;
    let mut tx = state.control.store().pool().begin().await?;
    let update=sqlx::query("UPDATE users SET password_hash=$1, auth_revision=auth_revision+1, updated_at=now() WHERE id=$2 AND auth_revision=$3 AND status='active'")
        .bind(hash).bind(me.id).bind(me.auth_revision).execute(&mut *tx).await?;
    if update.rows_affected() != 1 {
        return Err(WebError::new(StatusCode::CONFLICT, "conflict"));
    }
    sqlx::query("DELETE FROM web_sessions WHERE user_id=$1")
        .bind(me.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO web_admin_events(actor_id,action,resource_id) VALUES ($1,'account.password_changed',$1)")
        .bind(me.id).execute(&mut *tx).await?;
    tx.commit().await?;
    logout(State(state.clone()), headers).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cookie_parser_rejects_duplicates_and_invalid_tokens() {
        let mut headers = HeaderMap::new();
        assert!(token_hash(&headers).is_none());
        headers.insert(
            header::COOKIE,
            format!("{COOKIE}={}", "a".repeat(64)).parse().unwrap(),
        );
        assert_eq!(token_hash(&headers).unwrap().len(), 64);
        headers.insert(
            header::COOKIE,
            format!("{COOKIE}={}; {COOKIE}={}", "a".repeat(64), "b".repeat(64))
                .parse()
                .unwrap(),
        );
        assert!(token_hash(&headers).is_none());
    }
}
