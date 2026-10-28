use super::{
    WebError, WebState,
    management::{PageQuery, administrator},
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn list(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Value>, WebError> {
    administrator(&state, &headers).await?;
    let (page, size, search) = query.values()?;
    let mut tx = state.control.store().begin_snapshot(true).await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1)")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if !exists {
        return Err(WebError::new(StatusCode::NOT_FOUND, "not_found"));
    }
    // Include tombstones: this is an administrator's view of all added devices,
    // separate from the personal catalog's verification and mutation contracts.
    let base = "FROM user_saved_devices s JOIN devices d ON d.id=s.device_id WHERE s.user_id=$1 AND (position(lower($2) in lower(s.saved_name))>0 OR position(lower($2) in lower(s.alias))>0 OR position(replace($2,' ','') in d.code::text)>0)";
    let total: i64 = sqlx::query_scalar(&format!("SELECT count(*) {base}"))
        .bind(id)
        .bind(search)
        .fetch_one(&mut *tx)
        .await?;
    let rows = sqlx::query(&format!("SELECT d.id,d.code,s.saved_name,s.saved_system,s.alias,s.deleted,
        (extract(epoch from s.created_at)*1000)::bigint created_at,
        (extract(epoch from s.updated_at)*1000)::bigint updated_at,
        EXISTS(SELECT 1 FROM endpoints e WHERE e.device_id=d.id AND e.status='active' AND e.endpoint_key=ANY($3::bytea[])) online
        {base} ORDER BY s.created_at DESC,d.id LIMIT $4 OFFSET $5"))
        .bind(id).bind(search).bind(state.control.online_endpoint_keys()).bind(size).bind((page-1)*size)
        .fetch_all(&mut *tx).await?;
    let items = rows.into_iter().map(|r| Ok(json!({
        "id":r.try_get::<Uuid,_>("id")?, "code":format!("{:09}", r.try_get::<i32,_>("code")?),
        "name":r.try_get::<String,_>("saved_name")?, "system":r.try_get::<Option<String>,_>("saved_system")?,
        "alias":r.try_get::<String,_>("alias")?, "deleted":r.try_get::<bool,_>("deleted")?,
        "created_at":r.try_get::<i64,_>("created_at")?, "updated_at":r.try_get::<i64,_>("updated_at")?,
        "online":r.try_get::<bool,_>("online")?
    }))).collect::<Result<Vec<_>,sqlx::Error>>()?;
    tx.commit().await?;
    Ok(Json(
        json!({"items":items,"total":total,"page":page,"page_size":size}),
    ))
}
