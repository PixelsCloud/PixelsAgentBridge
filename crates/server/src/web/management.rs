use super::{
    WebError, WebState,
    session::{Viewer, viewer},
};
use crate::audit::{AuditAction, AuditRecord, AuditResource, AuditValues};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn administrator(
    state: &WebState,
    headers: &HeaderMap,
) -> Result<Viewer, WebError> {
    let me = if headers.contains_key(axum::http::header::AUTHORIZATION) {
        super::session::native_viewer(state, headers).await?
    } else {
        viewer(state, headers).await?
    };
    if !me.server_admin {
        return Err(WebError::new(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok(me)
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageQuery {
    pub(super) page: Option<u32>,
    pub(super) page_size: Option<u32>,
    pub(super) q: Option<String>,
}
impl PageQuery {
    pub(super) fn values(&self) -> Result<(i64, i64, &str), WebError> {
        let page = self.page.unwrap_or(1);
        let size = self.page_size.unwrap_or(20);
        let search = self.q.as_deref().unwrap_or("").trim();
        if page == 0 || page > 1_000_000 || size == 0 || size > 100 || search.chars().count() > 128
        {
            return Err(WebError::invalid());
        }
        Ok((page.into(), size.into(), search))
    }
}

async fn paged(
    state: &WebState,
    me: &Viewer,
    query: PageQuery,
    sql: &str,
) -> Result<Json<Value>, WebError> {
    let (page, size, search) = query.values()?;
    let mut tx = state.control.store().begin_snapshot(true).await?;
    let total: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM ({sql}) visible"))
        .bind(me.server_admin)
        .bind(me.id)
        .bind(search)
        .fetch_one(&mut *tx)
        .await?;
    let rows = sqlx::query(&format!("{sql} LIMIT $4 OFFSET $5"))
        .bind(me.server_admin)
        .bind(me.id)
        .bind(search)
        .bind(size)
        .bind((page - 1) * size)
        .fetch_all(&mut *tx)
        .await?;
    let items = rows
        .into_iter()
        .map(|row| {
            let text: String = row.try_get("payload")?;
            serde_json::from_str::<Value>(&text)
                .map_err(|_| WebError::new(StatusCode::INTERNAL_SERVER_ERROR, "server_error"))
        })
        .collect::<Result<Vec<_>, WebError>>()?;
    tx.commit().await?;
    Ok(Json(
        json!({"items":items,"total":total,"page":page,"page_size":size}),
    ))
}

pub(crate) async fn accounts(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    paged(&state,&me,query,r#"SELECT jsonb_build_object('id',u.id,'username',u.username,'status',u.status,'server_admin',u.server_admin,'revision',u.auth_revision,'relay_limit_mbps',u.relay_limit_mbps,'created_at',(extract(epoch from u.created_at)*1000)::bigint)::text payload
      FROM users u
      WHERE $1 AND $2::uuid IS NOT NULL AND position(lower($3) in lower(u.username))>0 ORDER BY lower(u.username),u.id"#).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AccountUpdate {
    status: String,
    server_admin: bool,
    revision: i64,
}
pub(crate) async fn update_account(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<AccountUpdate>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    if !matches!(input.status.as_str(), "active" | "disabled") {
        return Err(WebError::invalid());
    }
    let mut tx = state.control.store().pool().begin().await?;
    // Same deployment-wide lock is shared by bootstrap and all administrator changes.
    sqlx::query("SELECT pg_advisory_xact_lock(26035001)")
        .execute(&mut *tx)
        .await?;
    let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND server_admin AND status='active' AND auth_revision=$2)")
        .bind(me.id).bind(me.auth_revision).fetch_one(&mut *tx).await?;
    if !allowed {
        return Err(WebError::new(StatusCode::FORBIDDEN, "forbidden"));
    }
    let row =
        sqlx::query("SELECT auth_revision,server_admin,status FROM users WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if row.try_get::<i64, _>("auth_revision")? != input.revision {
        return Err(WebError::new(StatusCode::CONFLICT, "conflict"));
    }
    if row.try_get::<bool, _>("server_admin")?
        && row.try_get::<String, _>("status")? == "active"
        && (!input.server_admin || input.status != "active")
    {
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM users WHERE server_admin AND status='active'")
                .fetch_one(&mut *tx)
                .await?;
        if count <= 1 {
            return Err(WebError::new(StatusCode::CONFLICT, "last_admin"));
        }
    }
    sqlx::query("UPDATE users SET status=$1,server_admin=$2,auth_revision=auth_revision+1,updated_at=now() WHERE id=$3").bind(&input.status).bind(input.server_admin).bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE server_settings SET policy_revision=policy_revision+1 WHERE singleton")
        .execute(&mut *tx)
        .await?;
    let mut record = AuditRecord::new(
        Some(me.id),
        AuditAction::AccountUpdated,
        AuditResource::Account(id),
    );
    record.request_id = super::audit_context::request_id(&headers);
    record.before = Some(AuditValues {
        status: Some(row.try_get("status")?),
        server_admin: Some(row.try_get("server_admin")?),
        revision: Some(input.revision),
        ..Default::default()
    });
    record.after = Some(AuditValues {
        status: Some(input.status),
        server_admin: Some(input.server_admin),
        revision: Some(input.revision + 1),
        ..Default::default()
    });
    state.control.store().insert_audit(&mut tx, record).await?;
    tx.commit().await?;
    state.control.web_changed();
    Ok(Json(json!({"revision":input.revision+1})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UserLimitUpdate {
    mbps: i32,
}

pub(crate) async fn update_user_limit(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<UserLimitUpdate>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    if !pab_protocol::RELAY_MBPS_OPTIONS.contains(&input.mbps) {
        return Err(WebError::invalid());
    }
    let mut tx = state.control.store().pool().begin().await?;
    let before: i32 =
        sqlx::query_scalar("SELECT relay_limit_mbps FROM users WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    let updated = sqlx::query("UPDATE users SET relay_limit_mbps=$1,updated_at=now() WHERE id=$2")
        .bind(input.mbps)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if updated.rows_affected() == 0 {
        return Err(WebError::new(StatusCode::NOT_FOUND, "not_found"));
    }
    sqlx::query("UPDATE server_settings SET policy_revision=policy_revision+1 WHERE singleton")
        .execute(&mut *tx)
        .await?;
    let mut record = AuditRecord::new(
        Some(me.id),
        AuditAction::BandwidthUpdated,
        AuditResource::Account(id),
    );
    record.request_id = super::audit_context::request_id(&headers);
    record.before = Some(AuditValues {
        mbps: Some(before),
        ..Default::default()
    });
    record.after = Some(AuditValues {
        mbps: Some(input.mbps),
        ..Default::default()
    });
    state.control.store().insert_audit(&mut tx, record).await?;
    tx.commit().await?;
    state.control.web_changed();
    Ok(Json(json!({"mbps":input.mbps})))
}

pub(crate) async fn traffic(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, WebError> {
    let me = viewer(&state, &headers).await?;
    let row = sqlx::query("SELECT u.relay_limit_mbps AS user_mbps FROM users u CROSS JOIN server_settings s WHERE u.id=$1 AND s.singleton")
        .bind(me.id).fetch_one(state.control.store().pool()).await?;
    Ok(Json(
        json!({"user_mbps":row.try_get::<i32,_>("user_mbps")?}),
    ))
}

pub(crate) async fn relays(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<super::relay_status::RelayQuery>,
) -> Result<Json<Value>, WebError> {
    administrator(&state, &headers).await?;
    Ok(Json(super::relay_status::snapshot(&state, query).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RelayStateInput {
    pub state: pab_protocol::RelayAdminState,
    pub operation_id: Option<Uuid>,
    #[serde(default)]
    pub allow_outage: bool,
}
pub(crate) async fn relay_state(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(node): Path<String>,
    Json(input): Json<RelayStateInput>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    let config = state
        .control
        .relay_config()
        .ok_or_else(|| WebError::new(StatusCode::CONFLICT, "relay_cluster_disabled"))?;
    if !pab_protocol::valid_relay_node_id(&node) {
        return Err(WebError::invalid());
    }
    let operation = state
        .control
        .store()
        .set_relay_state_audited(
            &node,
            input.state,
            input.operation_id.unwrap_or_else(Uuid::new_v4),
            me.id,
            input.allow_outage,
            config.maintenance_seconds,
            config.offline_seconds,
            Some(state.server_instance),
            super::audit_context::request_id(&headers),
        )
        .await
        .map_err(|e| match e {
            crate::StoreError::InvalidState(message)
                if message.contains("no healthy Relay backup") =>
            {
                WebError::new(StatusCode::CONFLICT, "relay_no_backup")
            }
            crate::StoreError::InvalidState(_) => {
                WebError::new(StatusCode::CONFLICT, "relay_not_ready")
            }
            other => WebError::from(crate::ServiceError::from(other)),
        })?;
    state.control.web_changed();
    Ok(Json(
        serde_json::to_value(operation).map_err(|_| WebError::invalid())?,
    ))
}
pub(crate) async fn relay_operation(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, WebError> {
    administrator(&state, &headers).await?;
    let operation = state
        .control
        .store()
        .relay_maintenance_operation_in_instance(id, Some(state.server_instance))
        .await
        .map_err(crate::ServiceError::from)?;
    Ok(Json(
        serde_json::to_value(operation).map_err(|_| WebError::invalid())?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RelayCapacityInput {
    capacity_mbps: u32,
    machine_ip: Option<String>,
}

pub(crate) async fn relay_capacity(
    State(state): State<WebState>, headers: HeaderMap,
    Path(node): Path<String>, Json(input): Json<RelayCapacityInput>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    if !pab_protocol::valid_relay_node_id(&node) || input.capacity_mbps == 0 || input.capacity_mbps > i32::MAX as u32 {
        return Err(WebError::invalid());
    }
    let machine_ip = input.machine_ip.map(|ip| {
        let ip = ip.trim();
        if ip.is_empty() { return Ok(String::new()); }
        ip.parse::<std::net::IpAddr>().map(|ip| ip.to_string()).map_err(|_| WebError::invalid())
    }).transpose()?;
    state.control.set_relay_capacity(&node, input.capacity_mbps, machine_ip, me.id, super::audit_context::request_id(&headers))
        .await.map_err(crate::ServiceError::from)?;
    Ok(Json(json!({"capacity_mbps":input.capacity_mbps})))
}

pub(crate) async fn reload_relays(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    let mut record = AuditRecord::new(Some(me.id), AuditAction::RelayReload, AuditResource::Server);
    record.request_id = super::audit_context::request_id(&headers);
    let control = state.control.clone();
    tokio::spawn(async move { control.reload_relay_config_audited(Some(record)).await })
        .await
        .map_err(|_| WebError::new(StatusCode::INTERNAL_SERVER_ERROR, "server_error"))?
        .map_err(crate::ServiceError::from)?;
    Ok(Json(json!({"reloaded":true})))
}
