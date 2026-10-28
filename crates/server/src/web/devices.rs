use super::{
    WebError, WebState,
    session::{Viewer, viewer},
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    q: Option<String>,
    status: Option<String>,
    system: Option<String>,
    scope: Option<String>,
    user: Option<Uuid>,
    architecture: Option<String>,
    version: Option<String>,
    version_state: Option<String>,
    sort: Option<String>,
    attention: Option<bool>,
    owner_search: Option<bool>,
}
impl DeviceQuery {
    fn validate(&self, me: &Viewer) -> Result<(i64, i64, bool), WebError> {
        let page = self.page.unwrap_or(1);
        let size = self.page_size.unwrap_or(20);
        if page == 0
            || page > 1_000_000
            || size == 0
            || size > 100
            || self.q.as_ref().is_some_and(|s| s.chars().count() > 128)
            || !matches!(self.status.as_deref(), None | Some("online" | "offline"))
            || !matches!(
                self.system.as_deref(),
                None | Some("windows" | "linux" | "macos" | "android" | "unknown")
            )
            || !matches!(
                self.architecture.as_deref(),
                None | Some("x86_64" | "aarch64" | "unknown")
            )
            || !matches!(
                self.version_state.as_deref(),
                None | Some("reported" | "unknown")
            )
            || self.version.as_ref().is_some_and(|v| {
                v.is_empty() || v.chars().count() > 64 || v.chars().any(char::is_control)
            })
            || (self.version.is_some() && self.version_state.as_deref() == Some("unknown"))
            || !matches!(self.scope.as_deref(), None | Some("mine" | "all"))
            || !matches!(self.sort.as_deref(), None | Some("reported"))
        {
            return Err(WebError::invalid());
        }
        let all = self.scope.as_deref() == Some("all");
        if all && !me.server_admin {
            return Err(WebError::new(StatusCode::FORBIDDEN, "forbidden"));
        }
        if self.user.is_some_and(|id| id != me.id) && !me.server_admin {
            return Err(WebError::new(StatusCode::FORBIDDEN, "forbidden"));
        }
        Ok((i64::from(page), i64::from(size), all))
    }
}

// Personal management is authorized by the current association, never control credentials.
const BASE: &str = r#"FROM devices d
 LEFT JOIN device_accounts p ON p.device_id=d.id
 LEFT JOIN users owner ON owner.id=p.user_id
 LEFT JOIN device_runtime r ON r.device_id=d.id AND r.tenant_id=d.tenant_id
 WHERE ($1 OR p.user_id=$2)
 AND (position(lower($3) in lower(d.name))>0 OR position(replace($3,' ','') in d.code::text)>0)
 AND ($4::text IS NULL OR ($4='online')=EXISTS(SELECT 1 FROM endpoints e WHERE e.device_id=d.id AND e.status='active' AND e.endpoint_key=ANY($5::bytea[])))
 AND ($6::text IS NULL OR CASE WHEN r.execution_context->>'os_family' IN ('windows','linux','macos','android') THEN r.execution_context->>'os_family' ELSE 'unknown' END=$6)
 AND ($7::uuid IS NULL OR p.user_id=$7)
 AND ($8::text IS NULL OR CASE WHEN r.execution_context->>'architecture' IN ('x86_64','aarch64') THEN r.execution_context->>'architecture' ELSE 'unknown' END=$8)
 AND ($9::text IS NULL OR r.client_version=$9)
 AND ($10::text IS NULL OR ($10='reported')=(NULLIF(btrim(r.client_version),'') IS NOT NULL))"#;
const FIELDS: &str = r#"d.id,d.code,d.name,d.revision,d.status,p.revision AS association_revision,COALESCE(p.user_id=$2,false) AS associated_with_me,
 r.execution_context->>'os_family' AS system,r.execution_context->>'os_name' AS os_name,
 r.execution_context->>'os_version' AS os_version,r.execution_context->>'architecture' AS architecture,
 r.agent_version,r.client_version,p.user_id AS associated_user_id,owner.username AS associated_username,
 r.device_id IS NOT NULL AS runtime_reported,(extract(epoch from r.accepted_at)*1000)::bigint AS runtime_accepted_at,
 (extract(epoch from d.created_at)*1000)::bigint AS created_at,
 (extract(epoch from d.last_online_at)*1000)::bigint AS last_online_at,
 EXISTS(SELECT 1 FROM endpoints e WHERE e.device_id=d.id AND e.status='active' AND e.endpoint_key=ANY($5::bytea[])) AS online"#;

fn device(row: sqlx::postgres::PgRow) -> Result<Value, sqlx::Error> {
    Ok(
        json!({"id":row.try_get::<Uuid,_>("id")?,"associated_with_me":row.try_get::<bool,_>("associated_with_me")?,"association_revision":row.try_get::<Option<i64>,_>("association_revision")?,"code":format!("{:09}",row.try_get::<i32,_>("code")?),
      "name":row.try_get::<String,_>("name")?,"revision":row.try_get::<i64,_>("revision")?,
      "status":row.try_get::<String,_>("status")?,"system":row.try_get::<Option<String>,_>("system")?,
      "os_name":row.try_get::<Option<String>,_>("os_name")?,"os_version":row.try_get::<Option<String>,_>("os_version")?,
      "architecture":row.try_get::<Option<String>,_>("architecture")?,"agent_version":row.try_get::<Option<String>,_>("agent_version")?,
      "client_version":row.try_get::<Option<String>,_>("client_version")?,
      "associated_user_id":row.try_get::<Option<Uuid>,_>("associated_user_id")?,"associated_username":row.try_get::<Option<String>,_>("associated_username")?,
      "diagnostics":{"runtime_reported":row.try_get::<bool,_>("runtime_reported")?,"reported_at":row.try_get::<Option<i64>,_>("runtime_accepted_at")?,"permissions":"not_reported","compatibility":"not_reported"},
      "created_at":row.try_get::<i64,_>("created_at")?,"last_online_at":row.try_get::<Option<i64>,_>("last_online_at")?,
      "online":row.try_get::<bool,_>("online")?}),
    )
}

pub(crate) async fn list(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<DeviceQuery>,
) -> Result<Json<Value>, WebError> {
    let me = viewer(&state, &headers).await?;
    let (page, size, all) = query.validate(&me)?;
    let keys = state.control.online_endpoint_keys();
    let search = query.q.as_deref().unwrap_or("").trim();
    let mut base = BASE.to_owned();
    if query.owner_search == Some(true) {
        base = base.replace(
            "position(replace($3,' ','') in d.code::text)>0)",
            "position(replace($3,' ','') in d.code::text)>0 OR position(lower($3) in lower(COALESCE(owner.username,'')))>0)",
        );
    }
    if query.attention == Some(true) {
        base.push_str(" AND (r.device_id IS NULL OR NOT EXISTS(SELECT 1 FROM endpoints e WHERE e.device_id=d.id AND e.status='active' AND e.endpoint_key=ANY($5::bytea[])))");
    }
    let order = if query.sort.as_deref() == Some("reported") {
        "r.accepted_at DESC NULLS LAST,lower(d.name),d.id"
    } else {
        "online DESC,lower(d.name),d.id"
    };
    let mut tx = state.control.store().begin_snapshot(true).await?;
    let total: i64 = sqlx::query_scalar(&format!("SELECT count(*) {base}"))
        .bind(all)
        .bind(me.id)
        .bind(search)
        .bind(&query.status)
        .bind(&keys)
        .bind(&query.system)
        .bind(query.user)
        .bind(&query.architecture)
        .bind(&query.version)
        .bind(&query.version_state)
        .fetch_one(&mut *tx)
        .await?;
    let rows = sqlx::query(&format!(
        "SELECT {FIELDS} {base} ORDER BY {order} LIMIT $11 OFFSET $12"
    ))
    .bind(all)
    .bind(me.id)
    .bind(search)
    .bind(&query.status)
    .bind(&keys)
    .bind(&query.system)
    .bind(query.user)
    .bind(&query.architecture)
    .bind(&query.version)
    .bind(&query.version_state)
    .bind(size)
    .bind((page - 1) * size)
    .fetch_all(&mut *tx)
    .await?;
    let items = rows
        .into_iter()
        .map(device)
        .collect::<Result<Vec<_>, _>>()?;
    tx.commit().await?;
    Ok(Json(
        json!({"items":items,"total":total,"page":page,"page_size":size}),
    ))
}

pub(crate) async fn detail(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, WebError> {
    let me = viewer(&state, &headers).await?;
    let query = DeviceQuery::default();
    let sql = format!("SELECT {FIELDS} {BASE} AND d.id=$11");
    let row = sqlx::query(&sql)
        .bind(me.server_admin)
        .bind(me.id)
        .bind("")
        .bind(&query.status)
        .bind(state.control.online_endpoint_keys())
        .bind(&query.system)
        .bind(query.user)
        .bind(&query.architecture)
        .bind(&query.version)
        .bind(&query.version_state)
        .bind(id)
        .fetch_one(state.control.store().pool())
        .await?;
    Ok(Json(device(row)?))
}

pub(crate) async fn diagnostics(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<DeviceQuery>,
) -> Result<Json<Value>, WebError> {
    let me = viewer(&state, &headers).await?;
    let (_, _, all) = query.validate(&me)?;
    let sql = format!(
        r#"WITH visible AS (SELECT
        CASE WHEN r.execution_context->>'os_family' IN ('windows','linux','macos','android') THEN r.execution_context->>'os_family' END system,
        CASE WHEN r.execution_context->>'architecture' IN ('x86_64','aarch64') THEN r.execution_context->>'architecture' END architecture,
        NULLIF(btrim(r.client_version),'') version,r.device_id IS NOT NULL reported,
        EXISTS(SELECT 1 FROM endpoints e WHERE e.device_id=d.id AND e.status='active' AND e.endpoint_key=ANY($5::bytea[])) online {BASE})
        SELECT jsonb_build_object('total',count(*),'online',count(*) FILTER(WHERE online),'offline',count(*) FILTER(WHERE NOT online),
        'reported',count(*) FILTER(WHERE reported),'not_reported',count(*) FILTER(WHERE NOT reported),
        'permissions_unknown',count(*),'compatibility_unknown',count(*),
        'sampled_at',(extract(epoch from clock_timestamp())*1000)::bigint,
        'systems',COALESCE((SELECT jsonb_agg(jsonb_build_object('value',value,'count',n) ORDER BY value NULLS LAST) FROM (SELECT system value,count(*) n FROM visible GROUP BY system) g),'[]'::jsonb),
        'architectures',COALESCE((SELECT jsonb_agg(jsonb_build_object('value',value,'count',n) ORDER BY value NULLS LAST) FROM (SELECT architecture value,count(*) n FROM visible GROUP BY architecture) g),'[]'::jsonb),
        'versions',COALESCE((SELECT jsonb_agg(jsonb_build_object('value',value,'count',n) ORDER BY value NULLS LAST) FROM (SELECT version value,count(*) n FROM visible GROUP BY version ORDER BY version NULLS LAST LIMIT 100) g),'[]'::jsonb),
        'versions_truncated',(SELECT count(*)>100 FROM (SELECT version FROM visible GROUP BY version) g)) FROM visible"#
    );
    let value: Value = sqlx::query_scalar(&sql)
        .bind(all)
        .bind(me.id)
        .bind(query.q.as_deref().unwrap_or("").trim())
        .bind(&query.status)
        .bind(state.control.online_endpoint_keys())
        .bind(&query.system)
        .bind(query.user)
        .bind(&query.architecture)
        .bind(&query.version)
        .bind(&query.version_state)
        .fetch_one(state.control.store().pool())
        .await?;
    Ok(Json(value))
}

pub(crate) async fn overview(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<DeviceQuery>,
) -> Result<Json<Value>, WebError> {
    let me = viewer(&state, &headers).await?;
    let (_, _, requested_all) = query.validate(&me)?;
    let all = requested_all || (query.scope.is_none() && me.server_admin);
    let row=sqlx::query(r#"WITH visible AS (SELECT d.*,
      EXISTS(SELECT 1 FROM endpoints e WHERE e.device_id=d.id AND e.status='active' AND e.endpoint_key=ANY($3::bytea[])) online
      FROM devices d LEFT JOIN device_accounts p ON p.device_id=d.id WHERE ($1 OR p.user_id=$2) AND ($4::uuid IS NULL OR p.user_id=$4))
      SELECT count(*) total,count(*) FILTER (WHERE online) online,count(*) FILTER (WHERE NOT online) offline FROM visible"#)
        .bind(all).bind(me.id).bind(state.control.online_endpoint_keys()).bind(query.user).fetch_one(state.control.store().pool()).await?;
    let mut result = json!({"total":row.try_get::<i64,_>("total")?,"online":row.try_get::<i64,_>("online")?,
        "offline":row.try_get::<i64,_>("offline")?});
    if all {
        let accounts = sqlx::query("SELECT count(*) total,count(*) FILTER (WHERE status='active') enabled,count(*) FILTER (WHERE status='disabled') disabled,count(*) FILTER (WHERE server_admin) administrators FROM users")
            .fetch_one(state.control.store().pool())
            .await?;
        let relays = super::relay_status::snapshot(&state, Default::default()).await?;
        result["accounts"] = json!(accounts.try_get::<i64, _>("total")?);
        result["account_summary"] = json!({
            "enabled": accounts.try_get::<i64, _>("enabled")?,
            "disabled": accounts.try_get::<i64, _>("disabled")?,
            "administrators": accounts.try_get::<i64, _>("administrators")?,
        });
        result["relays"] = relays["summary"]["configured"].clone();
        result["relay_status"] = json!({"summary":relays["summary"],"sampled_at":relays["sampled_at"],"mode":relays["mode"],"offline_seconds":relays["offline_seconds"],"semantics_version":relays["semantics_version"],"health":relays["health"]});
    }
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Rename {
    name: String,
    revision: i64,
}
pub(crate) async fn rename(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<Rename>,
) -> Result<Json<Value>, WebError> {
    let me = viewer(&state, &headers).await?;
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 128 || name.chars().any(char::is_control) {
        return Err(WebError::invalid());
    }
    let mut tx = state.control.store().pool().begin().await?;
    // Serialize with unlink/reassociation, then authorize from a fresh READ COMMITTED snapshot.
    sqlx::query("SELECT id FROM devices WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    let row=sqlx::query("SELECT d.revision,d.name FROM devices d LEFT JOIN device_accounts p ON p.device_id=d.id WHERE d.id=$1 AND ($2 OR p.user_id=$3)")
        .bind(id).bind(me.server_admin).bind(me.id).fetch_one(&mut *tx).await?;
    if row.try_get::<i64, _>("revision")? != input.revision {
        return Err(WebError::new(StatusCode::CONFLICT, "conflict"));
    }
    sqlx::query("UPDATE devices SET name=$1,revision=revision+1,updated_at=now() WHERE id=$2")
        .bind(name)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let mut record = crate::audit::AuditRecord::new(
        Some(me.id),
        crate::audit::AuditAction::DeviceRenamed,
        crate::audit::AuditResource::Device(id),
    );
    record.request_id = super::audit_context::request_id(&headers);
    record.before = Some(crate::audit::AuditValues {
        name: Some(row.try_get("name")?),
        revision: Some(input.revision),
        ..Default::default()
    });
    record.after = Some(crate::audit::AuditValues {
        name: Some(name.into()),
        revision: Some(input.revision + 1),
        ..Default::default()
    });
    state.control.store().insert_audit(&mut tx, record).await?;
    tx.commit().await?;
    state.control.web_changed();
    Ok(Json(json!({"revision":input.revision+1})))
}
