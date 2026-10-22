use super::{WebError, WebState, session};
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
};
use pab_protocol::UsageBatch;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

mod export;
mod query;
mod ranking;
pub(crate) use export::export;
use query::{METRICS, Selection, exact_counters, legacy_counters};
pub(crate) use ranking::ranking;

pub(crate) async fn report(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(batch): Json<UsageBatch>,
) -> Result<StatusCode, WebError> {
    let me = session::native_viewer(&state, &headers).await?;
    if batch.user_id.map(|v| v.as_uuid()) != Some(me.id)
        || batch.counters.relay_upload_bytes != 0
        || batch.counters.relay_download_bytes != 0
        || (!batch.counters.p2p_reported
            && (batch.counters.p2p_upload_bytes != 0 || batch.counters.p2p_download_bytes != 0))
    {
        return Err(WebError::new(StatusCode::FORBIDDEN, "forbidden"));
    }
    state
        .control
        .store()
        .record_usage("client", &me.id.to_string(), &batch)
        .await
        .map_err(crate::ServiceError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UsageQuery {
    from: Option<i64>,
    until: Option<i64>,
    scope: Option<String>,
    user: Option<uuid::Uuid>,
    interval: Option<String>,
    population: Option<String>,
    page: Option<i64>,
    page_size: Option<i64>,
    sort: Option<String>,
    order: Option<String>,
}

mod summary;
pub(crate) use summary::summary;
