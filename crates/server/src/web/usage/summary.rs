use super::*;

pub(crate) async fn summary(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<UsageQuery>,
) -> Result<Json<Value>, WebError> {
    let me = session::viewer(&state, &headers).await?;
    let now = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    let selection = Selection::new(&query, &me, now)?;
    let mut tx = state.control.store().begin_snapshot(true).await?;
    sqlx::query("SET LOCAL statement_timeout='10s'")
        .execute(&mut *tx)
        .await?;
    let sql = format!(
        "WITH selected AS ({}), totals AS (
        SELECT hour_unix_ms,key,sum(value::numeric)::text total FROM selected
        CROSS JOIN LATERAL jsonb_each_text(counters) WHERE key IN ({})
        GROUP BY GROUPING SETS ((hour_unix_ms,key),(key))
      ), flags AS (
        SELECT hour_unix_ms,bool_or(COALESCE((counters->>'incomplete')::boolean,false)) incomplete,
          bool_or(COALESCE((counters->>'p2p_reported')::boolean,false)) p2p_reported,
          max((extract(epoch FROM updated_at)*1000)::bigint) updated FROM selected
        GROUP BY GROUPING SETS ((hour_unix_ms),())
      ) SELECT t.hour_unix_ms,jsonb_object_agg(t.key,t.total) counters,f.incomplete,f.updated,f.p2p_reported
        FROM totals t JOIN flags f ON t.hour_unix_ms IS NOT DISTINCT FROM f.hour_unix_ms
        GROUP BY t.hour_unix_ms,f.incomplete,f.updated,f.p2p_reported ORDER BY t.hour_unix_ms NULLS FIRST",
        selection.selected(),
        query::metric_list()
    );
    let rows = selection.query(&sql).fetch_all(&mut *tx).await?;
    let gap = selection.relay_gap(&mut tx).await?;
    tx.commit().await?;
    let mut exact = exact_counters(None);
    let mut incomplete = gap;
    let mut updated = None::<i64>;
    let mut p2p_reported = false;
    let mut points = Vec::new();
    for row in rows {
        let counters = exact_counters(Some(row.try_get("counters")?));
        let flag = row.try_get::<bool, _>("incomplete")?;
        let reported = row.try_get::<bool, _>("p2p_reported")?;
        let timestamp: Option<i64> = row.try_get("updated")?;
        if let Some(hour) = row.try_get::<Option<i64>, _>("hour_unix_ms")? {
            let (legacy, saturated) = legacy_counters(&counters, flag);
            points.push(
                json!({"hour_unix_ms":hour,"counters":legacy,"counters_exact":counters,
                "incomplete":flag,"p2p_reported":reported,"compatibility_saturated":saturated,"updated_at":timestamp}),
            );
        } else {
            exact = counters;
            incomplete |= flag;
            updated = timestamp;
            p2p_reported = reported;
        }
    }
    let (legacy, saturated) = legacy_counters(&exact, incomplete);
    let mut result = selection.metadata();
    result.as_object_mut().unwrap().extend(
        serde_json::from_value::<serde_json::Map<String, Value>>(json!({
            "counters":legacy,"counters_exact":exact,"points":points,"updated_at":updated,
            "incomplete":incomplete,"p2p_reported":p2p_reported,"compatibility_saturated":saturated,"client_reported":true
        }))
        .unwrap(),
    );
    Ok(Json(result))
}
