use super::*;
use axum::{body::Body, http::header, response::Response};
use std::convert::Infallible;

const ROW_LIMIT: usize = 10_000;

// Quote every field and neutralize spreadsheet formulas, including whitespace
// prefixes. Only a fixed numeric/metadata whitelist is ever exported.
fn cell(value: &str) -> String {
    let formula = value.trim_start().starts_with(['=', '+', '-', '@'])
        || value.starts_with(['\t', '\r', '\n']);
    format!(
        "\"{}{}\"",
        if formula { "'" } else { "" },
        value.replace('"', "\"\"")
    )
}
fn line(values: Vec<String>) -> String {
    values.iter().map(|v| cell(v)).collect::<Vec<_>>().join(",") + "\r\n"
}

pub(crate) async fn export(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<UsageQuery>,
) -> Result<Response, WebError> {
    let me = session::viewer(&state, &headers).await?;
    let now = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    let selection = Selection::new(&query, &me, now)?;
    let permit = state
        .exports
        .clone()
        .try_acquire_owned()
        .map_err(|_| WebError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited"))?;
    let mut tx = state.control.store().begin_snapshot(false).await?;
    sqlx::query("SET LOCAL statement_timeout='10s'")
        .execute(&mut *tx)
        .await?;
    // Bounded snapshot, one extra row solely to determine truncation before any
    // response is sent. CSV rows are encoded lazily into the response stream.
    let sql = format!("WITH selected AS ({}) SELECT s.*,u.username FROM selected s LEFT JOIN users u ON u.id::text=s.subject
        ORDER BY s.hour_unix_ms,s.subject,s.source_kind LIMIT 10001",selection.selected());
    let mut rows = selection.query(&sql).fetch_all(&mut *tx).await?;
    let truncated = rows.len() > ROW_LIMIT;
    rows.truncate(ROW_LIMIT);
    let row_count = rows.len();
    let gap = selection.relay_gap(&mut tx).await?;
    let metadata = selection.metadata();
    let mut record = crate::audit::AuditRecord::new(
        Some(me.id),
        crate::audit::AuditAction::UsageExport,
        if selection.all {
            crate::audit::AuditResource::Server
        } else {
            crate::audit::AuditResource::Account(query.user.unwrap_or(me.id))
        },
    );
    record.request_id = crate::web::audit_context::request_id(&headers);
    record.details = crate::audit::AuditDetails {
        from: metadata["from"].as_i64(),
        until: metadata["until"].as_i64(),
        interval: Some(selection.interval.into()),
        scope: Some(if selection.all { "all" } else { "account" }.into()),
        population: Some(selection.population.clone()),
        rows: Some(row_count),
        truncated: Some(truncated),
        ..Default::default()
    };
    state.control.store().insert_audit(&mut tx, record).await?;
    // Decode the bounded snapshot before committing the audit or HTTP headers.
    let records = rows
        .into_iter()
        .map(|row| -> Result<_, sqlx::Error> {
            Ok((
                row.try_get::<i64, _>("hour_unix_ms")?,
                row.try_get::<String, _>("source_kind")?,
                row.try_get::<String, _>("subject")?,
                row.try_get::<Option<String>, _>("username")?,
                row.try_get::<Value, _>("counters")?,
                row.try_get::<time::OffsetDateTime, _>("updated_at")?,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    tx.commit().await?;
    let mut names = [
        "record_type",
        "from_unix_ms",
        "until_unix_ms",
        "interval",
        "timezone",
        "bucket_unix_ms",
        "source_kind",
        "subject",
        "username",
        "updated_at_unix_ms",
        "incomplete",
    ]
    .map(String::from)
    .to_vec();
    names.extend(METRICS.map(String::from));
    names.extend(["rows".into(), "truncated".into()]);
    let first = "\u{feff}".to_owned() + &line(names);
    let prefix = vec![
        "data".into(),
        if selection.interval == "lifetime" {
            String::new()
        } else {
            selection.from.to_string()
        },
        if selection.interval == "lifetime" {
            String::new()
        } else {
            selection.until.to_string()
        },
        selection.interval.into(),
        "UTC".into(),
    ];
    let mut footer = prefix.clone();
    footer[0] = "export_status".into();
    footer.extend(std::iter::repeat_n(String::new(), 6 + METRICS.len()));
    footer[10] = (gap
        || records
            .iter()
            .any(|record| record.4["incomplete"].as_bool().unwrap_or(false)))
    .to_string();
    footer.extend([row_count.to_string(), truncated.to_string()]);
    let chunks = std::iter::once(first)
        .chain(records.into_iter().map(
            move |(bucket, source, subject, username, counters, updated)| {
                let mut fields = prefix.clone();
                fields.extend([
                    bucket.to_string(),
                    source,
                    subject,
                    username.unwrap_or_default(),
                    (updated.unix_timestamp_nanos() / 1_000_000).to_string(),
                    (gap || counters["incomplete"].as_bool().unwrap_or(false)).to_string(),
                ]);
                fields.extend(METRICS.map(|key| {
                    if key.starts_with("p2p_")
                        && !counters["p2p_reported"].as_bool().unwrap_or(false)
                    {
                        String::new()
                    } else {
                        counters[key].as_u64().unwrap_or(0).to_string()
                    }
                }));
                fields.extend([String::new(), String::new()]);
                line(fields)
            },
        ))
        .chain(std::iter::once(line(footer)));
    let stream = futures_util::stream::iter(chunks.map(move |chunk| {
        let _held = &permit;
        Ok::<_, Infallible>(chunk)
    }));
    Response::builder()
        .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"pixels-usage.csv\"",
        )
        .header("x-usage-truncated", truncated.to_string())
        .header("x-usage-rows", row_count.to_string())
        .header("x-usage-row-limit", ROW_LIMIT.to_string())
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream))
        .map_err(|_| WebError::invalid())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn csv_quotes_and_neutralizes_formulas() {
        assert_eq!(cell("中文,\"name\""), "\"中文,\"\"name\"\"\"");
        for value in [
            "=SUM(A1)", " +cmd", "-1+2", "@SUM(A1)", "\tplain", "\nplain",
        ] {
            assert!(cell(value).starts_with("\"'"));
        }
        assert_eq!(cell("9007199254740993"), "\"9007199254740993\"");
    }
}
