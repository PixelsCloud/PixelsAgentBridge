//! One scope/window contract shared by summaries, ranking and CSV.
use super::*;
use sqlx::{Postgres, postgres::PgArguments, query::Query};

pub(super) const HOUR: i64 = 3_600_000;
pub(super) const DAY: i64 = 86_400_000;
pub(super) const METRICS: [&str; 12] = [
    "relay_upload_bytes",
    "relay_download_bytes",
    "p2p_upload_bytes",
    "p2p_download_bytes",
    "connections",
    "connection_ms",
    "uploaded_files",
    "downloaded_files",
    "uploaded_bytes",
    "downloaded_bytes",
    "failed_transfers",
    "cancelled_transfers",
];

pub(super) struct Selection {
    pub table: &'static str,
    pub interval: &'static str,
    pub from: i64,
    pub until: i64,
    pub all: bool,
    pub subject: String,
    pub population: String,
    pub available_from: Option<i64>,
}

impl Selection {
    pub fn new(query: &UsageQuery, me: &session::Viewer, now: i64) -> Result<Self, WebError> {
        if !matches!(query.scope.as_deref(), None | Some("mine" | "all"))
            || !matches!(
                query.population.as_deref(),
                None | Some("all" | "users" | "guests")
            )
        {
            return Err(WebError::invalid());
        }
        let all = query.scope.as_deref() == Some("all");
        if (all || query.user.is_some_and(|id| id != me.id) || query.population.is_some())
            && !me.server_admin
        {
            return Err(WebError::new(StatusCode::FORBIDDEN, "forbidden"));
        }
        let requested_from = query.from.unwrap_or(now - now % DAY);
        let requested_until = query.until.unwrap_or(now - now % HOUR + HOUR);
        let interval = match query.interval.as_deref() {
            None | Some("hour") => "hour",
            Some("day") => "day",
            Some("lifetime") => "lifetime",
            Some("auto") => {
                if requested_from < available_from(now, 90, HOUR)
                    || requested_until.saturating_sub(requested_from) > 90 * DAY
                {
                    "day"
                } else {
                    "hour"
                }
            }
            _ => return Err(WebError::invalid()),
        };
        let granularity = if interval == "day" { DAY } else { HOUR };
        if interval != "lifetime"
            && (requested_from < 0
                || requested_until <= requested_from
                || requested_until > now + DAY)
        {
            return Err(WebError::invalid());
        }
        let automatic = query.interval.as_deref() == Some("auto");
        let from = if automatic {
            requested_from.div_euclid(granularity) * granularity
        } else {
            requested_from
        };
        let until = if automatic || (interval == "day" && query.until.is_none()) {
            requested_until.div_euclid(granularity) * granularity
                + i64::from(requested_until % granularity != 0) * granularity
        } else {
            requested_until
        };
        let retained = if interval == "lifetime" {
            None
        } else {
            Some(available_from(
                now,
                if interval == "day" { 730 } else { 90 },
                granularity,
            ))
        };
        if interval != "lifetime" {
            if from < 0
                || until <= from
                || from % granularity != 0
                || until % granularity != 0
                || until > now - now % granularity + granularity
            {
                return Err(WebError::invalid());
            }
            if from < retained.unwrap() {
                return Err(WebError::new(StatusCode::GONE, "usage_range_unavailable"));
            }
        }
        Ok(Self {
            table: match interval {
                "day" => "usage_daily",
                "lifetime" => "usage_lifetime",
                _ => "usage_hourly",
            },
            interval,
            from,
            until,
            all: all && query.user.is_none(),
            subject: query.user.unwrap_or(me.id).to_string(),
            population: query.population.clone().unwrap_or_else(|| "all".into()),
            available_from: retained,
        })
    }

    pub fn selected(&self) -> String {
        // Table names are selected above; all caller data remains bound parameters.
        format!("SELECT * FROM {} WHERE ($5 OR (hour_unix_ms>=$1 AND hour_unix_ms<$2))
            AND ($3 OR subject=$4) AND ($6='all' OR ($6='users' AND subject<>'guest') OR ($6='guests' AND subject='guest'))", self.table)
    }

    pub fn query<'q>(&self, sql: &'q str) -> Query<'q, Postgres, PgArguments> {
        sqlx::query(sql)
            .bind(self.from)
            .bind(self.until)
            .bind(self.all)
            .bind(self.subject.clone())
            .bind(self.interval == "lifetime")
            .bind(self.population.clone())
    }

    pub fn metadata(&self) -> Value {
        let lifetime = self.interval == "lifetime";
        json!({"from":(!lifetime).then_some(self.from),"until":(!lifetime).then_some(self.until),
            "interval":self.interval,"timezone":"UTC","population":self.population,
            "retention":{"hour_days":90,"day_days":730,"lifetime":true,"available_from":self.available_from}})
    }

    pub async fn relay_gap(
        &self,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<bool, WebError> {
        // Only disclose the unattributed gap flag, never guest or other-user counters.
        Ok(sqlx::query_scalar(&format!("SELECT EXISTS(SELECT 1 FROM {} WHERE source_kind='relay' AND subject='guest'
            AND ($3 OR (hour_unix_ms>=$1 AND hour_unix_ms<$2)) AND COALESCE((counters->>'incomplete')::boolean,false))", self.table))
            .bind(self.from).bind(self.until).bind(self.interval == "lifetime").fetch_one(&mut **tx).await?)
    }
}

fn available_from(now: i64, days: i64, bucket: i64) -> i64 {
    // The writer prunes against wall clock time, including a partial oldest bucket.
    let cutoff = now - days * DAY;
    (cutoff.div_euclid(bucket) + i64::from(cutoff % bucket != 0)) * bucket
}

pub(super) fn exact_counters(value: Option<Value>) -> Value {
    let mut counters = serde_json::Map::new();
    for metric in METRICS {
        counters.insert(
            metric.into(),
            value
                .as_ref()
                .and_then(|v| v.get(metric))
                .cloned()
                .unwrap_or_else(|| json!("0")),
        );
    }
    Value::Object(counters)
}

pub(super) fn legacy_counters(exact: &Value, incomplete: bool) -> (Value, bool) {
    let mut counters = serde_json::Map::new();
    let mut saturated = false;
    for metric in METRICS {
        let number = exact[metric]
            .as_str()
            .unwrap_or("0")
            .parse::<u64>()
            .unwrap_or_else(|_| {
                saturated = true;
                u64::MAX
            });
        counters.insert(metric.into(), json!(number));
    }
    counters.insert("incomplete".into(), json!(incomplete || saturated));
    (Value::Object(counters), saturated)
}

pub(super) fn metric_list() -> String {
    METRICS.map(|key| format!("'{key}'")).join(",")
}
