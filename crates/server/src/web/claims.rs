use super::{WebError, WebState, session::viewer};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewClaim {
    device_code: String,
    request_id: Uuid,
}
pub(crate) async fn begin(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(input): Json<NewClaim>,
) -> Result<Json<Value>, WebError> {
    let me = viewer(&state, &headers).await?;
    let code = input.device_code.replace(' ', "");
    if code.len() != 9 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return Err(WebError::invalid());
    }
    let code: i32 = code.parse().map_err(|_| WebError::invalid())?;
    if !(100000000..=999999999).contains(&code) {
        return Err(WebError::invalid());
    }
    let mut tx = state.control.store().pool().begin().await?;
    // Serialize this account's submissions and the requested device's ownership.
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(me.id)
        .execute(&mut *tx)
        .await?;
    if let Some(row)=sqlx::query("SELECT c.id,d.code,c.requested_by_user_id FROM device_claim_requests c JOIN devices d ON d.id=c.device_id WHERE c.id=$1")
        .bind(input.request_id).fetch_optional(&mut *tx).await? {
        if row.try_get::<Uuid,_>("requested_by_user_id")?!=me.id||row.try_get::<i32,_>("code")?!=code{return Err(WebError::new(StatusCode::CONFLICT,"conflict"));}
        return Ok(Json(json!({"id":input.request_id})));
    }
    let device = sqlx::query(
        "SELECT id,owner_tenant_id FROM devices WHERE code=$1 AND status='active' FOR UPDATE",
    )
    .bind(code)
    .fetch_one(&mut *tx)
    .await?;
    if device
        .try_get::<Option<Uuid>, _>("owner_tenant_id")?
        .is_some()
    {
        return Err(WebError::new(StatusCode::CONFLICT, "already_claimed"));
    }
    let device_id: Uuid = device.try_get("id")?;
    let pending:i64=sqlx::query_scalar("SELECT count(*) FROM device_claim_requests WHERE (device_id=$1 OR requested_by_user_id=$2) AND resolution='pending' AND expires_at>now()")
        .bind(device_id).bind(me.id).fetch_one(&mut *tx).await?;
    if pending >= 20 {
        return Err(WebError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited"));
    }
    sqlx::query("INSERT INTO device_claim_requests(id,device_id,owner_tenant_id,requested_by_user_id,expires_at) VALUES($1,$2,$3,$4,now()+interval '10 minutes')")
        .bind(input.request_id).bind(device_id).bind(me.personal_tenant_id).bind(me.id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO web_admin_events(actor_id,action,resource_id) VALUES($1,'device.claim_requested',$2)").bind(me.id).bind(device_id).execute(&mut *tx).await?;
    tx.commit().await?;
    state.control.web_changed();
    Ok(Json(json!({"id":input.request_id})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClaimPage {
    page: Option<u32>,
    page_size: Option<u32>,
}
pub(crate) async fn list(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(input): Query<ClaimPage>,
) -> Result<Json<Value>, WebError> {
    let me = viewer(&state, &headers).await?;
    let page = input.page.unwrap_or(1);
    let size = input.page_size.unwrap_or(20);
    if page == 0 || page > 1_000_000 || size == 0 || size > 100 {
        return Err(WebError::invalid());
    }
    let mut tx = state.control.store().pool().begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM device_claim_requests WHERE requested_by_user_id=$1",
    )
    .bind(me.id)
    .fetch_one(&mut *tx)
    .await?;
    let rows=sqlx::query(r#"SELECT c.id,d.code,CASE WHEN d.owner_tenant_id=c.owner_tenant_id OR (d.owner_tenant_id IS NULL AND c.resolution='pending' AND c.expires_at>now()) THEN d.name ELSE NULL END name,c.resolution,
       c.expires_at<=now() expired,(extract(epoch from c.expires_at)*1000)::bigint expires,
       (extract(epoch from c.created_at)*1000)::bigint created
       FROM device_claim_requests c JOIN devices d ON d.id=c.device_id WHERE c.requested_by_user_id=$1
       ORDER BY c.created_at DESC,c.id LIMIT $2 OFFSET $3"#).bind(me.id).bind(i64::from(size)).bind(i64::from((page-1)*size)).fetch_all(&mut *tx).await?;
    let items=rows.into_iter().map(|r|->Result<Value,sqlx::Error>{
        let resolution:String=r.try_get("resolution")?;let status=if resolution=="pending"&&r.try_get::<bool,_>("expired")?{"expired"}else{&resolution};
        Ok(json!({"id":r.try_get::<Uuid,_>("id")?,"device_code":format!("{:09}",r.try_get::<i32,_>("code")?),"name":r.try_get::<Option<String>,_>("name")?,"status":status,"expires_at":r.try_get::<i64,_>("expires")?,"created_at":r.try_get::<i64,_>("created")?}))
    }).collect::<Result<Vec<_>,_>>()?;
    tx.commit().await?;
    Ok(Json(
        json!({"items":items,"total":total,"page":page,"page_size":size}),
    ))
}

pub(crate) async fn cancel(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, WebError> {
    let me = viewer(&state, &headers).await?;
    let mut tx = state.control.store().pool().begin().await?;
    let row=sqlx::query("SELECT device_id,resolution FROM device_claim_requests WHERE id=$1 AND requested_by_user_id=$2 AND resolution IN ('pending','cancelled') AND expires_at>now() FOR UPDATE")
        .bind(id).bind(me.id).fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        return Err(WebError::new(StatusCode::CONFLICT, "claim_not_pending"));
    };
    if row.try_get::<String, _>("resolution")? == "pending" {
        sqlx::query("UPDATE device_claim_requests SET resolution='cancelled' WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO web_admin_events(actor_id,action,resource_id) VALUES($1,'device.claim_cancelled',$2)").bind(me.id).bind(row.try_get::<Uuid,_>("device_id")?).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    state.control.web_changed();
    Ok(Json(json!({"success":true})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Unbind {
    revision: i64,
}
pub(crate) async fn unbind(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<Unbind>,
) -> Result<Json<Value>, WebError> {
    let me = viewer(&state, &headers).await?;
    let mut tx = state.control.store().pool().begin().await?;
    let row=sqlx::query("SELECT d.revision FROM devices d LEFT JOIN personal_tenants p ON p.tenant_id=d.owner_tenant_id WHERE d.id=$1 AND d.owner_tenant_id IS NOT NULL AND ($2 OR p.user_id=$3) FOR UPDATE OF d")
        .bind(id).bind(me.server_admin).bind(me.id).fetch_one(&mut *tx).await?;
    if row.try_get::<i64, _>("revision")? != input.revision {
        return Err(WebError::new(StatusCode::CONFLICT, "conflict"));
    }
    sqlx::query(
        "UPDATE devices SET owner_tenant_id=NULL,revision=revision+1,updated_at=now() WHERE id=$1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE device_claim_requests SET resolution='cancelled' WHERE device_id=$1 AND resolution='pending'").bind(id).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO web_admin_events(actor_id,action,resource_id) VALUES($1,'device.unbound',$2)",
    )
    .bind(me.id)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    state.control.web_changed();
    Ok(Json(json!({"success":true})))
}
