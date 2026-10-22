use super::{WebError, WebState, audit_context, management::administrator};
use crate::audit::{AuditAction, AuditDetails, AuditRecord, AuditResource, AuditValues};
use axum::{
    Json,
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct AuditQuery {
    from: Option<i64>,
    until: Option<i64>,
    actor: Option<Uuid>,
    action: Option<String>,
    result: Option<String>,
    resource_kind: Option<String>,
    resource: Option<String>,
    q: Option<String>,
    page_size: Option<u32>,
    cursor: Option<String>,
    // Accept the old first-page URL, but never emulate unstable OFFSET pages.
    page: Option<u32>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    snapshot: String,
    anchor: i64,
    bound: i64,
    after: bool,
    filter: String,
}

fn encode(cursor: &Cursor) -> String {
    serde_json::to_vec(cursor)
        .unwrap()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn decode(text: &str) -> Result<Cursor, WebError> {
    if text.len() > 32768 || text.len() % 2 != 0 {
        return Err(WebError::invalid());
    }
    let bytes = (0..text.len())
        .step_by(2)
        .map(|index| {
            text.get(index..index + 2)
                .and_then(|s| u8::from_str_radix(s, 16).ok())
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(WebError::invalid)?;
    let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| WebError::invalid())?;
    if cursor.anchor < 0
        || cursor.bound < 0
        || cursor.snapshot.len() > 8192
        || !cursor
            .snapshot
            .bytes()
            .all(|c| c.is_ascii_digit() || matches!(c, b':' | b','))
        || cursor.snapshot.matches(':').count() != 2
    {
        return Err(WebError::invalid());
    }
    Ok(cursor)
}

struct Selection {
    query: AuditQuery,
    size: i64,
    filter: String,
}
impl Selection {
    fn new(mut query: AuditQuery) -> Result<Self, WebError> {
        let size = query.page_size.unwrap_or(20);
        if !(1..=100).contains(&size)
            || query
                .from
                .is_some_and(|t| !(0..=253402300799999).contains(&t))
            || query
                .until
                .is_some_and(|t| !(0..=253402300799999).contains(&t))
            || query
                .from
                .zip(query.until)
                .is_some_and(|(from, until)| until <= from)
            || query.action.as_ref().is_some_and(|v| {
                v.len() > 64
                    || !v
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.'))
            })
            || query.result.as_deref().is_some_and(|v| {
                !matches!(v, "succeeded" | "denied" | "failed" | "historical_unknown")
            })
            || query.resource_kind.as_deref().is_some_and(|v| {
                !matches!(
                    v,
                    "account" | "device" | "relay" | "server" | "audit" | "unknown"
                )
            })
            || query
                .resource
                .as_ref()
                .is_some_and(|v| v.chars().count() > 128 || v.chars().any(char::is_control))
            || query
                .q
                .as_ref()
                .is_some_and(|v| v.chars().count() > 128 || v.chars().any(char::is_control))
            || query.page.is_some_and(|p| p != 1)
        {
            return Err(WebError::invalid());
        }
        query.q = query.q.take().filter(|q| !q.is_empty());
        let mut identity = query.clone();
        identity.cursor = None;
        identity.page = None;
        let filter = blake3::hash(&serde_json::to_vec(&identity).unwrap())
            .to_hex()
            .to_string();
        Ok(Self {
            query,
            size: size.into(),
            filter,
        })
    }
    fn conditions(&self) -> &'static str {
        "FROM web_admin_events e WHERE e.id<=$1 AND (e.insert_xid IS NULL OR pg_visible_in_snapshot(e.insert_xid,$2::pg_snapshot))
        AND ($3::bigint IS NULL OR e.created_at>=to_timestamp($3::double precision/1000))
        AND ($4::bigint IS NULL OR e.created_at<to_timestamp($4::double precision/1000))
        AND ($5::uuid IS NULL OR COALESCE(e.actor_ref,e.actor_id)=$5)
        AND ($6::text IS NULL OR e.action=$6) AND ($7::text IS NULL OR e.result=$7)
        AND ($8::text IS NULL OR e.resource_kind=$8) AND ($9::text IS NULL OR COALESCE(e.resource_key,e.resource_id::text)=$9)
        AND ($10::text IS NULL OR position(lower($10) in lower(COALESCE(e.actor_name,'')||' '||COALESCE(e.resource_name,'')||' '||e.action||' '||COALESCE(COALESCE(e.actor_ref,e.actor_id)::text,'')))>0)"
    }
    fn bind<'a>(
        &self,
        sql: &'a str,
        cursor: &Cursor,
    ) -> sqlx::query::Query<'a, sqlx::Postgres, sqlx::postgres::PgArguments> {
        // Optional filters need a plan for their current values. Reusing a
        // generic plan makes warmed, concurrent audit reads substantially slower.
        sqlx::query(sql)
            .persistent(false)
            .bind(cursor.anchor)
            .bind(cursor.snapshot.clone())
            .bind(self.query.from)
            .bind(self.query.until)
            .bind(self.query.actor)
            .bind(self.query.action.clone())
            .bind(self.query.result.clone())
            .bind(self.query.resource_kind.clone())
            .bind(self.query.resource.clone())
            .bind(self.query.q.clone())
    }
    async fn cursor(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<Cursor, WebError> {
        if let Some(text) = &self.query.cursor {
            let cursor = decode(text)?;
            if cursor.filter != self.filter {
                return Err(WebError::new(
                    StatusCode::BAD_REQUEST,
                    "audit_cursor_mismatch",
                ));
            }
            // Let PostgreSQL validate its snapshot representation; never interpolate it.
            sqlx::query("SELECT $1::pg_snapshot")
                .bind(&cursor.snapshot)
                .execute(&mut **tx)
                .await
                .map_err(|_| WebError::invalid())?;
            Ok(cursor)
        } else {
            let row=sqlx::query("SELECT pg_current_snapshot()::text snapshot,COALESCE(max(id),0) anchor FROM web_admin_events").fetch_one(&mut **tx).await?;
            Ok(Cursor {
                snapshot: row.try_get("snapshot")?,
                anchor: row.try_get("anchor")?,
                bound: 0,
                after: false,
                filter: self.filter.clone(),
            })
        }
    }
}

fn payload(row: sqlx::postgres::PgRow) -> Result<Value, WebError> {
    let version: i16 = row.try_get("format_version")?;
    let decode_values =
        |value: Option<Value>| value.and_then(|v| serde_json::from_value::<AuditValues>(v).ok());
    let before = if version == 1 {
        decode_values(row.try_get("before_values")?)
    } else {
        None
    };
    let after = if version == 1 {
        decode_values(row.try_get("after_values")?)
    } else {
        None
    };
    let details = if version == 1 {
        row.try_get::<Option<Value>, _>("details")?
            .and_then(|v| serde_json::from_value::<AuditDetails>(v).ok())
    } else {
        None
    };
    let created: time::OffsetDateTime = row.try_get("created_at")?;
    Ok(
        json!({"id":format!("web:{}",row.try_get::<i64,_>("id")?),"actor":row.try_get::<Option<String>,_>("actor_name")?,
        "actor_id":row.try_get::<Option<Uuid>,_>("actor_ref")?.or(row.try_get("actor_id")?),"action":row.try_get::<String,_>("action")?,
        "resource_id":row.try_get::<Option<Uuid>,_>("resource_id")?,"resource_kind":row.try_get::<Option<String>,_>("resource_kind")?,
        "resource_key":row.try_get::<Option<String>,_>("resource_key")?,"resource_name":row.try_get::<Option<String>,_>("resource_name")?,
        "created_at":(created.unix_timestamp_nanos()/1_000_000) as i64,"request_id":row.try_get::<Option<Uuid>,_>("request_id")?,
        "operation_id":row.try_get::<Option<Uuid>,_>("operation_id")?,"result":row.try_get::<String,_>("result")?,
        "change_state":if version==0 {"historical_unknown"}else if before.is_none()&&after.is_none(){"not_applicable"}else{"recorded"},
        "before":before,"after":after,"details":details}),
    )
}

pub(crate) async fn list(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Value>, WebError> {
    administrator(&state, &headers).await?;
    let selection = Selection::new(query)?;
    let mut tx = state.control.store().begin_snapshot(true).await?;
    sqlx::query("SET LOCAL statement_timeout='10s'")
        .execute(&mut *tx)
        .await?;
    let cursor = selection.cursor(&mut tx).await?;
    let sql = format!(
        "SELECT count(*) total,min(id) minimum,max(id) maximum {}",
        selection.conditions()
    );
    let count = selection.bind(&sql, &cursor).fetch_one(&mut *tx).await?;
    let minimum: Option<i64> = count.try_get("minimum")?;
    let maximum: Option<i64> = count.try_get("maximum")?;
    let sql = format!(
        "SELECT e.* {} AND ($11::bigint=0 OR {} $11) ORDER BY e.id {} LIMIT $12",
        selection.conditions(),
        if cursor.after { "e.id>" } else { "e.id<" },
        if cursor.after { "ASC" } else { "DESC" }
    );
    let mut rows = selection
        .bind(&sql, &cursor)
        .bind(cursor.bound)
        .bind(selection.size)
        .fetch_all(&mut *tx)
        .await?;
    if cursor.after {
        rows.reverse();
    }
    let first = rows.first().map(|r| r.get::<i64, _>("id"));
    let last = rows.last().map(|r| r.get::<i64, _>("id"));
    let previous = first
        .filter(|first| maximum.is_some_and(|max| max > *first))
        .map(|bound| {
            encode(&Cursor {
                bound,
                after: true,
                ..Cursor {
                    snapshot: cursor.snapshot.clone(),
                    anchor: cursor.anchor,
                    bound: 0,
                    after: false,
                    filter: cursor.filter.clone(),
                }
            })
        });
    let next = last
        .filter(|last| minimum.is_some_and(|min| min < *last))
        .map(|bound| {
            encode(&Cursor {
                bound,
                after: false,
                ..Cursor {
                    snapshot: cursor.snapshot.clone(),
                    anchor: cursor.anchor,
                    bound: 0,
                    after: false,
                    filter: cursor.filter.clone(),
                }
            })
        });
    let items = rows
        .into_iter()
        .map(payload)
        .collect::<Result<Vec<_>, _>>()?;
    tx.commit().await?;
    let start = encode(&Cursor {
        bound: 0,
        after: false,
        ..cursor
    });
    Ok(Json(
        json!({"items":items,"total":count.try_get::<i64,_>("total")?,"page":1,"page_size":selection.size,
        "next_cursor":next,"previous_cursor":previous,"snapshot_cursor":start,"snapshot_frozen":true}),
    ))
}

fn csv_cell(value: &str) -> String {
    format!(
        "\"{}{}\"",
        if value.trim_start().starts_with(['=', '+', '-', '@'])
            || value.starts_with(['\t', '\r', '\n'])
        {
            "'"
        } else {
            ""
        },
        value.replace('"', "\"\"")
    )
}
fn csv_line(values: Vec<String>) -> String {
    values
        .iter()
        .map(|v| csv_cell(v))
        .collect::<Vec<_>>()
        .join(",")
        + "\r\n"
}
pub(crate) async fn export(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<AuditQuery>,
) -> Result<Response, WebError> {
    let me = administrator(&state, &headers).await?;
    let selection = Selection::new(query)?;
    let permit = state
        .exports
        .clone()
        .try_acquire_owned()
        .map_err(|_| WebError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited"))?;
    let mut tx = state.control.store().begin_snapshot(false).await?;
    sqlx::query("SET LOCAL statement_timeout='10s'")
        .execute(&mut *tx)
        .await?;
    let cursor = selection.cursor(&mut tx).await?;
    // Export the entire frozen filtered view, not just the currently displayed page.
    let sql = format!(
        "SELECT e.* {} ORDER BY e.id DESC LIMIT 10001",
        selection.conditions()
    );
    let mut rows = selection.bind(&sql, &cursor).fetch_all(&mut *tx).await?;
    let truncated = rows.len() > 10000;
    rows.truncate(10000);
    let count = rows.len();
    let items = rows
        .into_iter()
        .map(payload)
        .collect::<Result<Vec<_>, _>>()?;
    let mut record = AuditRecord::new(Some(me.id), AuditAction::AuditExport, AuditResource::Audit);
    record.request_id = audit_context::request_id(&headers);
    record.details.rows = Some(count);
    record.details.truncated = Some(truncated);
    record.details.filters_applied = Some(
        selection.query.from.is_some()
            || selection.query.until.is_some()
            || selection.query.actor.is_some()
            || selection.query.action.is_some()
            || selection.query.result.is_some()
            || selection.query.resource.is_some()
            || selection.query.resource_kind.is_some()
            || selection.query.q.is_some(),
    );
    state.control.store().insert_audit(&mut tx, record).await?;
    tx.commit().await?;
    let names = [
        "record_type",
        "id",
        "created_at_unix_ms",
        "actor_id",
        "actor_name",
        "action",
        "result",
        "resource_kind",
        "resource_key",
        "resource_name",
        "request_id",
        "operation_id",
        "change_state",
        "before",
        "after",
        "details",
        "rows",
        "truncated",
    ];
    let first = "\u{feff}".to_owned() + &csv_line(names.map(String::from).to_vec());
    let mut footer = vec![String::new(); names.len()];
    footer[0] = "export_status".into();
    footer[16] = count.to_string();
    footer[17] = truncated.to_string();
    let chunks = std::iter::once(first)
        .chain(items.into_iter().map(move |item| {
            let mut values = vec!["data".into()];
            for key in &names[1..16] {
                let value = &item[*key];
                values.push(if value.is_null() {
                    String::new()
                } else if let Some(text) = value.as_str() {
                    text.into()
                } else {
                    value.to_string()
                });
            }
            // The API uses created_at; CSV names the millisecond unit explicitly.
            values[2] = item["created_at"].to_string();
            values[4] = item["actor"].as_str().unwrap_or_default().into();
            values.extend([String::new(), String::new()]);
            csv_line(values)
        }))
        .chain(std::iter::once(csv_line(footer)));
    let stream = futures_util::stream::iter(chunks.map(move |chunk| {
        let _held = &permit;
        Ok::<_, std::convert::Infallible>(chunk)
    }));
    Response::builder()
        .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"pixels-audit.csv\"",
        )
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-audit-truncated", truncated.to_string())
        .header("x-audit-rows", count.to_string())
        .body(Body::from_stream(stream))
        .map_err(|_| WebError::invalid())
}
