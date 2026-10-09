use super::{WebError, WebState, session};
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
};
use pab_protocol::{
    DeviceId, DeviceRef, SavedDevice, SavedDeviceChanges, SavedDeviceMutation, TenantId,
};
use serde::Deserialize;
use sqlx::Row;
use uuid::Uuid;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Changes {
    after: Option<i64>,
    limit: Option<i64>,
}

async fn read(
    state: WebState,
    user: Uuid,
    query: Changes,
) -> Result<Json<SavedDeviceChanges>, WebError> {
    let after = query.after.unwrap_or(0);
    let limit = query.limit.unwrap_or(200);
    if after < 0 || !(1..=500).contains(&limit) {
        return Err(WebError::invalid());
    }
    let rows=sqlx::query("SELECT s.*,d.tenant_id,d.code,CASE WHEN s.verified_until>now() AND NOT s.deleted THEN EXISTS(SELECT 1 FROM endpoints e WHERE e.device_id=d.id AND e.status='active' AND e.endpoint_key=ANY($4::bytea[])) END online FROM user_saved_devices s JOIN devices d ON d.id=s.device_id WHERE s.user_id=$1 AND s.revision>$2 ORDER BY s.revision LIMIT $3")
        .bind(user).bind(after).bind(limit+1).bind(state.control.online_endpoint_keys()).fetch_all(state.control.store().pool()).await?;
    let has_more = rows.len() > limit as usize;
    let items = rows
        .into_iter()
        .take(limit as usize)
        .map(|r| {
            Ok(SavedDevice {
                device_ref: DeviceRef {
                    device_id: DeviceId::from_uuid(r.try_get("device_id")?),
                    tenant_id: TenantId::from_uuid(r.try_get("tenant_id")?),
                },
                code: format!("{:09}", r.try_get::<i32, _>("code")?),
                name: r.try_get("saved_name")?,
                system: r.try_get("saved_system")?,
                alias: r.try_get("alias")?,
                revision: r.try_get("revision")?,
                deleted: r.try_get("deleted")?,
                online: r.try_get("online")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let cursor = items.last().map_or(after, |v| v.revision);
    Ok(Json(SavedDeviceChanges {
        items,
        cursor,
        has_more,
    }))
}

async fn mutate(
    state: WebState,
    user: Uuid,
    input: SavedDeviceMutation,
) -> Result<Json<serde_json::Value>, WebError> {
    if input.alias.chars().count() > 128
        || input.alias.chars().any(char::is_control)
        || input.expected_revision <= 0
    {
        return Err(WebError::invalid());
    }
    let request = serde_json::to_value(&input).map_err(|_| WebError::invalid())?;
    let mut tx = state.control.store().pool().begin().await?;
    sqlx::query("SELECT user_id FROM user_catalog_versions WHERE user_id=$1 FOR UPDATE")
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    if let Some(row) =
        sqlx::query("SELECT request,result FROM user_catalog_mutations WHERE user_id=$1 AND id=$2")
            .bind(user)
            .bind(input.id)
            .fetch_optional(&mut *tx)
            .await?
    {
        if row.try_get::<serde_json::Value, _>("request")? != request {
            return Err(WebError::new(StatusCode::CONFLICT, "mutation_reused"));
        }
        return Ok(Json(row.try_get("result")?));
    }
    let current=sqlx::query("SELECT revision,deleted,verified_until>now() AS verified FROM user_saved_devices WHERE user_id=$1 AND device_id=$2")
        .bind(user).bind(input.device_id.as_uuid()).fetch_one(&mut *tx).await?;
    if current.try_get::<i64, _>("revision")? != input.expected_revision {
        return Err(WebError::new(StatusCode::CONFLICT, "catalog_changed"));
    }
    if current.try_get::<bool, _>("deleted")?
        && !input.deleted
        && !current.try_get::<bool, _>("verified")?
    {
        return Err(WebError::new(
            StatusCode::FORBIDDEN,
            "device_verification_required",
        ));
    }
    let revision: i64 = sqlx::query_scalar(
        "UPDATE user_catalog_versions SET revision=revision+1 WHERE user_id=$1 RETURNING revision",
    )
    .bind(user)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE user_saved_devices SET alias=$1,deleted=$2,revision=$3,updated_at=now() WHERE user_id=$4 AND device_id=$5")
        .bind(&input.alias).bind(input.deleted).bind(revision).bind(user).bind(input.device_id.as_uuid()).execute(&mut *tx).await?;
    let result = serde_json::json!({"revision":revision});
    sqlx::query(
        "INSERT INTO user_catalog_mutations(user_id,id,request,result) VALUES($1,$2,$3,$4)",
    )
    .bind(user)
    .bind(input.id)
    .bind(request)
    .bind(&result)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    state.control.web_changed();
    Ok(Json(result))
}

pub(crate) async fn web_changes(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<Changes>,
) -> Result<Json<SavedDeviceChanges>, WebError> {
    let me = session::viewer(&state, &headers).await?;
    read(state, me.id, query).await
}
pub(crate) async fn native_changes(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<Changes>,
) -> Result<Json<SavedDeviceChanges>, WebError> {
    let me = session::native_viewer(&state, &headers).await?;
    read(state, me.id, query).await
}
pub(crate) async fn web_mutate(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(input): Json<SavedDeviceMutation>,
) -> Result<Json<serde_json::Value>, WebError> {
    let me = session::viewer(&state, &headers).await?;
    mutate(state, me.id, input).await
}
pub(crate) async fn native_mutate(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(input): Json<SavedDeviceMutation>,
) -> Result<Json<serde_json::Value>, WebError> {
    let me = session::native_viewer(&state, &headers).await?;
    mutate(state, me.id, input).await
}

pub(crate) async fn native_import(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(input): Json<pab_protocol::SavedDeviceImport>,
) -> Result<Json<serde_json::Value>, WebError> {
    let me = session::native_viewer(&state, &headers).await?;
    let code = input
        .code
        .parse::<pab_protocol::DeviceCode>()
        .map_err(|_| WebError::invalid())?;
    if [&input.name, &input.alias]
        .into_iter()
        .any(|s| s.chars().count() > 128 || s.chars().any(char::is_control))
        || !matches!(
            input.system.as_deref(),
            None | Some("windows" | "macos" | "linux")
        )
    {
        return Err(WebError::invalid());
    }
    let mut tx = state.control.store().pool().begin().await?;
    sqlx::query("SELECT id FROM devices WHERE id=$1 AND tenant_id=$2 AND code::text=$3")
        .bind(input.device_ref.device_id.as_uuid())
        .bind(input.device_ref.tenant_id.as_uuid())
        .bind(code.to_string().trim_start_matches('0'))
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO user_catalog_versions(user_id) VALUES($1) ON CONFLICT DO NOTHING")
        .bind(me.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT user_id FROM user_catalog_versions WHERE user_id=$1 FOR UPDATE")
        .bind(me.id)
        .fetch_one(&mut *tx)
        .await?;
    // Repeating an import never overwrites an existing alias or revives a tombstone.
    if let Some(revision) = sqlx::query_scalar::<_, i64>(
        "SELECT revision FROM user_saved_devices WHERE user_id=$1 AND device_id=$2",
    )
    .bind(me.id)
    .bind(input.device_ref.device_id.as_uuid())
    .fetch_optional(&mut *tx)
    .await?
    {
        return Ok(Json(serde_json::json!({"revision":revision})));
    }
    let revision: i64 = sqlx::query_scalar(
        "UPDATE user_catalog_versions SET revision=revision+1 WHERE user_id=$1 RETURNING revision",
    )
    .bind(me.id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO user_saved_devices(user_id,device_id,revision,saved_name,saved_system,alias,verified_until) VALUES($1,$2,$3,$4,$5,$6,'epoch')")
        .bind(me.id).bind(input.device_ref.device_id.as_uuid()).bind(revision).bind(input.name).bind(input.system).bind(input.alias).execute(&mut *tx).await?;
    tx.commit().await?;
    state.control.web_changed();
    Ok(Json(serde_json::json!({"revision":revision})))
}
