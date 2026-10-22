use super::*;

pub(crate) async fn ranking(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<UsageQuery>,
) -> Result<Json<Value>, WebError> {
    let me = session::viewer(&state, &headers).await?;
    if !me.server_admin {
        return Err(WebError::new(StatusCode::FORBIDDEN, "forbidden"));
    }
    let now = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    let selection = Selection::new(&query, &me, now)?;
    let page = query.page.unwrap_or(1);
    let size = query.page_size.unwrap_or(20);
    let metric = query.sort.as_deref().unwrap_or("relay_download_bytes");
    let order = query.order.as_deref().unwrap_or("desc");
    if !(1..=1_000_000).contains(&page)
        || !(1..=100).contains(&size)
        || !METRICS.contains(&metric)
        || !matches!(order, "asc" | "desc")
    {
        return Err(WebError::invalid());
    }
    let mut tx = state.control.store().begin_snapshot(true).await?;
    sqlx::query("SET LOCAL statement_timeout='10s'")
        .execute(&mut *tx)
        .await?;
    let count_sql = format!(
        "WITH selected AS ({}) SELECT count(*) total FROM (SELECT subject FROM selected GROUP BY subject) s",
        selection.selected()
    );
    let total: i64 = selection
        .query(&count_sql)
        .fetch_one(&mut *tx)
        .await?
        .try_get("total")?;
    let sql = format!("WITH selected AS ({}), totals AS (
        SELECT subject,key,sum(value::numeric) total FROM selected
        CROSS JOIN LATERAL jsonb_each_text(counters) WHERE key IN ({}) GROUP BY subject,key
      ), amounts AS (
        SELECT subject,jsonb_object_agg(key,total::text) counters,
          COALESCE(max(total) FILTER (WHERE key='{metric}'),0) sort_value FROM totals GROUP BY subject
      ), flags AS (
        SELECT subject,bool_or(COALESCE((counters->>'incomplete')::boolean,false)) incomplete,
          bool_or(COALESCE((counters->>'p2p_reported')::boolean,false)) p2p_reported,
          max((extract(epoch FROM updated_at)*1000)::bigint) updated FROM selected GROUP BY subject
      ) SELECT a.subject,u.id user_id,u.username,a.counters,f.incomplete,f.updated,f.p2p_reported
        FROM amounts a JOIN flags f USING(subject) LEFT JOIN users u ON u.id::text=a.subject
        ORDER BY a.sort_value {order},a.subject ASC LIMIT $7 OFFSET $8",selection.selected(),query::metric_list());
    let rows = selection
        .query(&sql)
        .bind(size)
        .bind((page - 1) * size)
        .fetch_all(&mut *tx)
        .await?;
    let gap = selection.relay_gap(&mut tx).await?;
    tx.commit().await?;
    let items = rows.into_iter().map(|row| -> Result<Value,sqlx::Error> {
        let subject: String = row.try_get("subject")?;
        Ok(json!({"subject":subject,"user_id":row.try_get::<Option<uuid::Uuid>,_>("user_id")?,
            "username":row.try_get::<Option<String>,_>("username")?,"population":if subject=="guest" {"guests"} else {"users"},
            "counters_exact":exact_counters(Some(row.try_get("counters")?)),
            "p2p_reported":row.try_get::<bool,_>("p2p_reported")?,
            "incomplete":gap||row.try_get::<bool,_>("incomplete")?,"updated_at":row.try_get::<Option<i64>,_>("updated")?}))
    }).collect::<Result<Vec<_>,_>>()?;
    let mut result = selection.metadata();
    result.as_object_mut().unwrap().extend(serde_json::from_value::<serde_json::Map<String,Value>>(json!({
        "items":items,"total":total,"page":page,"page_size":size,"sort":metric,"order":order,"incomplete":gap
    })).unwrap());
    Ok(Json(result))
}
