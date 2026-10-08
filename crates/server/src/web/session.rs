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
// Browser cookie retention is capped by browsers; server sessions never expire.
const MAX_AGE: u64 = 400 * 24 * 60 * 60;

#[derive(Serialize)]
pub(crate) struct Viewer {
    pub id: Uuid,
    pub username: String,
    pub personal_tenant_id: Uuid,
    pub server_admin: bool,
    #[serde(skip)]
    pub auth_revision: i64,
}

fn cookie_token(headers: &HeaderMap) -> Option<&str> {
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
    Some(value)
}

fn token_hash(headers: &HeaderMap) -> Option<String> {
    cookie_token(headers).map(hash_token)
}

fn hash_token(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let token = values.next()?.to_str().ok()?.strip_prefix("Bearer ")?;
    if values.next().is_some() || token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    Some(token)
}

pub(crate) fn native_token_hash(headers: &HeaderMap) -> Option<String> {
    bearer_token(headers).map(hash_token)
}

fn cookie(token: &str) -> String {
    format!("{COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={MAX_AGE}")
}

pub(crate) async fn viewer(state: &WebState, headers: &HeaderMap) -> Result<Viewer, WebError> {
    let hash = token_hash(headers).ok_or_else(WebError::unauthorized)?;
    viewer_by_hash(state, &hash).await
}

pub(crate) async fn native_viewer(
    state: &WebState,
    headers: &HeaderMap,
) -> Result<Viewer, WebError> {
    let token = bearer_token(headers).ok_or_else(WebError::unauthorized)?;
    viewer_by_hash(state, &hash_token(token)).await
}

async fn viewer_by_hash(state: &WebState, hash: &str) -> Result<Viewer, WebError> {
    let row = sqlx::query("SELECT u.id, u.username, u.server_admin, u.auth_revision, p.tenant_id FROM web_sessions s JOIN users u ON u.id=s.user_id JOIN personal_tenants p ON p.user_id=u.id WHERE s.token_hash=$1 AND u.status='active'")
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
) -> Result<Response, WebError> {
    let me = viewer(&state, &headers).await?;
    let token = cookie_token(&headers).ok_or_else(WebError::unauthorized)?;
    Ok(([(header::SET_COOKIE, cookie(token))], Json(me)).into_response())
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
    Json(input): Json<Credentials>,
) -> Result<Response, WebError> {
    let _permit = login_permit(&state).await?;
    establish(&state, input, SessionChannel::Browser).await
}

#[derive(Clone, Copy)]
enum SessionChannel {
    Browser,
    Native,
}

async fn establish(
    state: &WebState,
    input: Credentials,
    channel: SessionChannel,
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
    // Serialize session creation with password changes without expiring other sessions.
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(account.id.as_uuid())
        .execute(&mut *tx)
        .await?;
    let inserted = sqlx::query("INSERT INTO web_sessions (token_hash,user_id) SELECT $1,id FROM users WHERE id=$2 AND auth_revision=$3 AND status='active'")
        .bind(&hash).bind(account.id.as_uuid()).bind(revision).execute(&mut *tx).await?;
    if inserted.rows_affected() != 1 {
        return Err(WebError::unauthorized());
    }
    tx.commit().await?;
    let me = viewer_by_hash(state, &hash).await?;
    Ok(match channel {
        SessionChannel::Browser => {
            ([(header::SET_COOKIE, cookie(&token))], Json(me)).into_response()
        }
        SessionChannel::Native => Json(json!({"user":me,"access_token":token})).into_response(),
    })
}

pub(crate) async fn register(
    State(state): State<WebState>,
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
    establish(&state, input, SessionChannel::Browser).await
}

pub(crate) async fn logout(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    if let Some(hash) = token_hash(&headers) {
        revoke_session(&state, &hash).await?;
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

pub(crate) async fn native_current(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Viewer>, WebError> {
    Ok(Json(native_viewer(&state, &headers).await?))
}

pub(crate) async fn native_login(
    State(state): State<WebState>,
    Json(input): Json<Credentials>,
) -> Result<Response, WebError> {
    let _permit = login_permit(&state).await?;
    establish(&state, input, SessionChannel::Native).await
}

pub(crate) async fn native_register(
    State(state): State<WebState>,
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
    establish(&state, input, SessionChannel::Native).await
}

pub(crate) async fn native_logout(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Result<StatusCode, WebError> {
    let token = bearer_token(&headers).ok_or_else(WebError::unauthorized)?;
    revoke_session(&state, &hash_token(token)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn revoke_session(state: &WebState, hash: &str) -> Result<(), WebError> {
    let mut tx = state.control.store().pool().begin().await?;
    let deleted = sqlx::query("DELETE FROM web_sessions WHERE token_hash=$1")
        .bind(hash)
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() > 0 {
        sqlx::query("UPDATE server_settings SET policy_revision=policy_revision+1 WHERE singleton")
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    state.control.web_changed();
    Ok(())
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
    sqlx::query("INSERT INTO web_admin_events(actor_id,action,resource_id) VALUES ($1,'account.password_changed',$1)")
        .bind(me.id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
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
