use super::{WebError, WebState, audit_context, management::administrator, session};
use crate::{
    audit::{AuditAction, AuditRecord, AuditResource, AuditValues},
    postgres::RegistrationPolicy,
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

#[derive(Deserialize)]
#[serde(untagged)]
enum RequiredOverride {
    Enabled(bool),
    Inherit(()),
}
impl RequiredOverride {
    fn value(self) -> Option<bool> {
        match self {
            Self::Enabled(value) => Some(value),
            Self::Inherit(()) => None,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PolicyUpdate {
    revision: i64,
    registration_override: RequiredOverride,
}
pub(super) async fn get(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, WebError> {
    administrator(&state, &headers).await?;
    let row=sqlx::query("SELECT registration_override,registration_revision,policy_revision FROM server_settings WHERE singleton")
        .fetch_one(state.control.store().pool()).await?;
    let policy = RegistrationPolicy::resolve(
        state.registration_enabled,
        row.try_get("registration_override")?,
        row.try_get("registration_revision")?,
    );
    Ok(Json(
        json!({"registration_enabled":policy.effective,"registration":policy,"version":env!("CARGO_PKG_VERSION"),"policy_revision":row.try_get::<i64,_>("policy_revision")?,"session_expires":false}),
    ))
}
pub(super) async fn update(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(input): Json<PolicyUpdate>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    if input.revision < 1 {
        return Err(WebError::invalid());
    }
    let hash = session::token_hash(&headers).ok_or_else(WebError::unauthorized)?;
    let mut tx = state.control.store().pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(26035001)")
        .execute(&mut *tx)
        .await?;
    let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u JOIN web_sessions s ON s.user_id=u.id WHERE u.id=$1 AND s.token_hash=$2 AND u.status='active' AND u.server_admin AND u.auth_revision=$3)")
        .bind(me.id).bind(hash).bind(me.auth_revision).fetch_one(&mut *tx).await?;
    if !allowed {
        return Err(WebError::new(StatusCode::FORBIDDEN, "forbidden"));
    }
    let row=sqlx::query("SELECT registration_override,registration_revision FROM server_settings WHERE singleton FOR UPDATE").fetch_one(&mut *tx).await?;
    let before = RegistrationPolicy::resolve(
        state.registration_enabled,
        row.try_get("registration_override")?,
        row.try_get("registration_revision")?,
    );
    if input.revision != before.revision {
        return Err(WebError::new(StatusCode::CONFLICT, "conflict"));
    }
    let after = RegistrationPolicy::resolve(
        state.registration_enabled,
        input.registration_override.value(),
        before.revision + 1,
    );
    sqlx::query("UPDATE server_settings SET registration_override=$1,registration_revision=$2,policy_revision=policy_revision+1 WHERE singleton")
        .bind(after.override_value).bind(after.revision).execute(&mut *tx).await?;
    let values = |p: &RegistrationPolicy| AuditValues {
        registration_enabled: Some(p.effective),
        registration_source: Some(p.source.into()),
        revision: Some(p.revision),
        ..Default::default()
    };
    let mut record = AuditRecord::new(
        Some(me.id),
        AuditAction::RegistrationUpdated,
        AuditResource::Server,
    );
    record.request_id = audit_context::request_id(&headers);
    record.before = Some(values(&before));
    record.after = Some(values(&after));
    state.control.store().insert_audit(&mut tx, record).await?;
    tx.commit().await?;
    state.control.web_changed();
    Ok(Json(
        json!({"registration_enabled":after.effective,"registration":after}),
    ))
}
