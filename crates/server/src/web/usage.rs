use super::{WebError, WebState, session};
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
};
use pab_protocol::{UsageBatch, UsageCounters};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

pub(crate) async fn report(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(batch): Json<UsageBatch>,
) -> Result<StatusCode, WebError> {
    let me = session::native_viewer(&state, &headers).await?;
    if batch.user_id.map(|v| v.as_uuid()) != Some(me.id)
        || batch.counters.relay_upload_bytes != 0
        || batch.counters.relay_download_bytes != 0
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UsageQuery {
    from: Option<i64>,
    until: Option<i64>,
    scope: Option<String>,
    user: Option<uuid::Uuid>,
    interval: Option<String>,
    population: Option<String>,
}

pub(crate) async fn summary(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<UsageQuery>,
) -> Result<Json<Value>, WebError> {
    let me = session::viewer(&state, &headers).await?;
    let now = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    let all = query.scope.as_deref() == Some("all");
    let (table, interval, max_days) = match query.interval.as_deref() {
        None | Some("hour") => ("usage_hourly", "hour", 90),
        Some("day") => ("usage_daily", "day", 730),
        Some("lifetime") => ("usage_lifetime", "lifetime", 90),
        _ => return Err(WebError::invalid()),
    };
    let granularity = if interval == "day" {
        86_400_000
    } else {
        3_600_000
    };
    let from = query.from.unwrap_or(now - now % 86_400_000);
    let until = query.until.unwrap_or(now - now % granularity + granularity);
    if !matches!(query.scope.as_deref(), None | Some("mine" | "all"))
        || from < 0
        || until <= from
        || until - from > max_days * 86_400_000
        || from % granularity != 0
        || until % granularity != 0
        || !matches!(
            query.population.as_deref(),
            None | Some("all" | "users" | "guests")
        )
    {
        return Err(WebError::invalid());
    }
    if (all || query.user.is_some_and(|id| id != me.id) || query.population.is_some())
        && !me.server_admin
    {
        return Err(WebError::new(StatusCode::FORBIDDEN, "forbidden"));
    }
    let subject = query.user.unwrap_or(me.id).to_string();
    let population = match query.population.as_deref() {
        Some("users") => "AND subject<>'guest'",
        Some("guests") => "AND subject='guest'",
        _ => "",
    };
    // Aggregate in SQL: response memory is bounded by the requested time buckets, not user count.
    let rows=sqlx::query(&format!("WITH selected AS (SELECT * FROM {table} WHERE ($5 OR (hour_unix_ms>=$1 AND hour_unix_ms<$2)) AND ($3 OR subject=$4) {population}), totals AS (SELECT hour_unix_ms,key,LEAST(sum(value::numeric),18446744073709551615) total, sum(value::numeric)>18446744073709551615 overflow FROM selected CROSS JOIN LATERAL jsonb_each_text(counters) WHERE key<>'incomplete' GROUP BY hour_unix_ms,key), sums AS (SELECT hour_unix_ms,jsonb_object_agg(key,total) counters,bool_or(overflow) overflow FROM totals GROUP BY hour_unix_ms), flags AS (SELECT hour_unix_ms,bool_or(COALESCE((counters->>'incomplete')::boolean,false)) incomplete,max((extract(epoch FROM updated_at)*1000)::bigint) updated FROM selected GROUP BY hour_unix_ms) SELECT s.hour_unix_ms,s.counters||jsonb_build_object('incomplete',f.incomplete OR s.overflow) counters,f.updated FROM sums s JOIN flags f USING(hour_unix_ms) ORDER BY hour_unix_ms"))
        .bind(from).bind(until).bind(all&&query.user.is_none()).bind(subject).bind(interval=="lifetime").fetch_all(state.control.store().pool()).await?;
    let mut total = UsageCounters::default();
    let mut points = std::collections::BTreeMap::<i64, UsageCounters>::new();
    let mut updated = None::<i64>;
    for row in rows {
        let value: UsageCounters =
            serde_json::from_value(row.try_get("counters")?).map_err(|_| WebError::invalid())?;
        total.add(&value);
        points
            .entry(row.try_get("hour_unix_ms")?)
            .or_default()
            .add(&value);
        updated = Some(updated.unwrap_or(0).max(row.try_get("updated")?));
    }
    let points: Vec<_> = points
        .into_iter()
        .map(|(hour, counters)| json!({"hour_unix_ms":hour,"counters":counters}))
        .collect();
    // A crashed Relay cannot attribute its lost final window. Expose only the gap flag,
    // never another user's counters, so personal totals cannot look falsely complete.
    let relay_gap: bool = sqlx::query_scalar(&format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE source_kind='relay' AND subject='guest' AND ($3 OR (hour_unix_ms>=$1 AND hour_unix_ms<$2)) AND COALESCE((counters->>'incomplete')::boolean,false))"))
        .bind(from).bind(until).bind(interval=="lifetime").fetch_one(state.control.store().pool()).await?;
    total.incomplete |= relay_gap;
    Ok(Json(
        json!({"from":from,"until":until,"counters":total,"points":points,"updated_at":updated,"interval":interval,"client_reported":true,"retention":{"hour_days":90,"day_days":730,"lifetime":true}}),
    ))
}
