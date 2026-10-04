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
                None | Some("windows" | "linux" | "macos")
            )
            || !matches!(self.scope.as_deref(), None | Some("mine" | "all"))
        {
            return Err(WebError::invalid());
        }
        let all = self.scope.as_deref() == Some("all");
        if all && !me.server_admin {
            return Err(WebError::new(StatusCode::FORBIDDEN, "forbidden"));
        }
        Ok((i64::from(page), i64::from(size), all))
    }
}

// Every row is authorized by CURRENT personal ownership, never claim history.
const BASE: &str = r#"FROM devices d
 LEFT JOIN personal_tenants p ON p.tenant_id=d.owner_tenant_id
 LEFT JOIN device_runtime r ON r.device_id=d.id AND r.tenant_id=d.tenant_id
 WHERE ($1 OR p.user_id=$2)
 AND (position(lower($3) in lower(d.name))>0 OR position(replace($3,' ','') in d.code::text)>0)
 AND ($4::text IS NULL OR ($4='online')=EXISTS(SELECT 1 FROM endpoints e WHERE e.device_id=d.id AND e.status='active' AND e.endpoint_key=ANY($5::bytea[])))
 AND ($6::text IS NULL OR r.execution_context->>'os_family'=$6)"#;
const FIELDS: &str = r#"d.id,d.code,d.name,d.revision,d.status,
 r.execution_context->>'os_family' AS system,r.execution_context->>'os_name' AS os_name,
 r.execution_context->>'os_version' AS os_version,r.execution_context->>'architecture' AS architecture,
 r.agent_version, (extract(epoch from d.created_at)*1000)::bigint AS created_at,
 (extract(epoch from d.last_online_at)*1000)::bigint AS last_online_at,
 EXISTS(SELECT 1 FROM endpoints e WHERE e.device_id=d.id AND e.status='active' AND e.endpoint_key=ANY($5::bytea[])) AS online"#;

fn device(row: sqlx::postgres::PgRow) -> Result<Value, sqlx::Error> {
    Ok(
        json!({"id":row.try_get::<Uuid,_>("id")?,"code":format!("{:09}",row.try_get::<i32,_>("code")?),
      "name":row.try_get::<String,_>("name")?,"revision":row.try_get::<i64,_>("revision")?,
      "status":row.try_get::<String,_>("status")?,"system":row.try_get::<Option<String>,_>("system")?,
      "os_name":row.try_get::<Option<String>,_>("os_name")?,"os_version":row.try_get::<Option<String>,_>("os_version")?,
      "architecture":row.try_get::<Option<String>,_>("architecture")?,"agent_version":row.try_get::<Option<String>,_>("agent_version")?,
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
    let mut tx = state.control.store().pool().begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let total: i64 = sqlx::query_scalar(&format!("SELECT count(*) {BASE}"))
        .bind(all)
        .bind(me.id)
        .bind(search)
        .bind(&query.status)
        .bind(&keys)
        .bind(&query.system)
        .fetch_one(&mut *tx)
        .await?;
    let rows = sqlx::query(&format!(
        "SELECT {FIELDS} {BASE} ORDER BY online DESC,lower(d.name),d.id LIMIT $7 OFFSET $8"
    ))
    .bind(all)
    .bind(me.id)
    .bind(search)
    .bind(&query.status)
    .bind(&keys)
    .bind(&query.system)
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
    let sql = format!("SELECT {FIELDS} {BASE} AND d.id=$7");
    let row = sqlx::query(&sql)
        .bind(me.server_admin)
        .bind(me.id)
        .bind("")
        .bind(&query.status)
        .bind(state.control.online_endpoint_keys())
        .bind(&query.system)
        .bind(id)
        .fetch_one(state.control.store().pool())
        .await?;
    Ok(Json(device(row)?))
}

pub(crate) async fn overview(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, WebError> {
    let me = viewer(&state, &headers).await?;
    let row=sqlx::query(r#"WITH visible AS (SELECT d.*,
      EXISTS(SELECT 1 FROM endpoints e WHERE e.device_id=d.id AND e.status='active' AND e.endpoint_key=ANY($3::bytea[])) online
      FROM devices d LEFT JOIN personal_tenants p ON p.tenant_id=d.owner_tenant_id WHERE ($1 OR p.user_id=$2))
      SELECT count(*) total,count(*) FILTER (WHERE online) online,count(*) FILTER (WHERE NOT online) offline FROM visible"#)
        .bind(me.server_admin).bind(me.id).bind(state.control.online_endpoint_keys()).fetch_one(state.control.store().pool()).await?;
    let mut result = json!({"total":row.try_get::<i64,_>("total")?,"online":row.try_get::<i64,_>("online")?,
        "offline":row.try_get::<i64,_>("offline")?});
    if me.server_admin {
        let counts = sqlx::query("SELECT (SELECT count(*) FROM users) accounts,(SELECT count(*) FROM teams JOIN tenants ON tenants.id=teams.tenant_id WHERE tenants.status='active') teams,(SELECT count(*) FROM relay_nodes WHERE server_instance=$1 AND last_seen_at>now()-interval '120 seconds') relays")
            .bind(state.server_instance).fetch_one(state.control.store().pool()).await?;
        for key in ["accounts", "teams", "relays"] {
            result[key] = json!(counts.try_get::<i64, _>(key)?);
        }
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
    let row=sqlx::query("SELECT d.revision FROM devices d LEFT JOIN personal_tenants p ON p.tenant_id=d.owner_tenant_id WHERE d.id=$1 AND ($2 OR p.user_id=$3) FOR UPDATE OF d")
        .bind(id).bind(me.server_admin).bind(me.id).fetch_one(&mut *tx).await?;
    if row.try_get::<i64, _>("revision")? != input.revision {
        return Err(WebError::new(StatusCode::CONFLICT, "conflict"));
    }
    sqlx::query("UPDATE devices SET name=$1,revision=revision+1,updated_at=now() WHERE id=$2")
        .bind(name)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO web_admin_events(actor_id,action,resource_id) VALUES ($1,'device.rename',$2)",
    )
    .bind(me.id)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    state.control.web_changed();
    Ok(Json(json!({"revision":input.revision+1})))
}
