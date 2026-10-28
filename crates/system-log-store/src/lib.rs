//! Durable, bounded per-device log scans. A native source checkpoint is
//! committed in the same SQLite transaction as every visible event batch.

use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::Engine as _;
use pab_os_system_log::NativePage;
use pab_protocol::{
    DeviceRef, OperatorRef, SystemLogData, SystemLogEvent, SystemLogFilter, SystemLogIntegrity,
    SystemLogMode, SystemLogSource, SystemLogSourceProgress,
};
use sha2::{Digest, Sha256};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    Row, SqlitePool,
};
use thiserror::Error;
use uuid::Uuid;

const MAX_SCAN_BYTES: i64 = 128 * 1024 * 1024;
const MAX_TOTAL_BYTES: i64 = 512 * 1024 * 1024;
const MAX_SCAN_EVENTS: i64 = 100_000;
const MAX_TOTAL_EVENTS: i64 = 250_000;
const MAX_ACTIVE_SCANS: i64 = 64;
const PAGE_BUDGET: usize = 20 * 1024;
const SOURCE_PROGRESS_LIMIT: i64 = 16;
const RETENTION_MS: i64 = 24 * 60 * 60 * 1_000;
const LAGGING_AGE_MS: u64 = 5_000;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("SQLite: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("serialization: {0}")]
    Json(#[from] serde_json::Error),
    #[error("scan or log reference not found for this caller")]
    NotFound,
    #[error("source checkpoint conflict")]
    CheckpointConflict,
    #[error("log scan storage budget exhausted")]
    Quota,
    #[error("invalid log scan request: {0}")]
    Invalid(&'static str),
}

#[derive(Clone)]
pub struct LogStore {
    pool: SqlitePool,
}

pub struct NewScan<'a> {
    pub actor: OperatorRef,
    pub device: DeviceRef,
    pub identity: &'a str,
    pub sources: &'a [SystemLogSource],
    pub start_unix_ms: i64,
    pub end_unix_ms: i64,
    pub mode: SystemLogMode,
    pub filter: Option<&'a SystemLogFilter>,
}

impl LogStore {
    /// Close this store and wait for SQLite connections to release their files.
    pub async fn shutdown(self) {self.pool.close().await;}
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|_| StoreError::Invalid("cannot create log storage directory"))?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await?;
        for statement in [
            "CREATE TABLE IF NOT EXISTS log_scans (id TEXT PRIMARY KEY, actor_key TEXT NOT NULL, device_json TEXT NOT NULL, identity TEXT NOT NULL, start_ms INTEGER NOT NULL, end_ms INTEGER NOT NULL, mode TEXT NOT NULL, filter_json TEXT, bytes_used INTEGER NOT NULL DEFAULT 0, events_used INTEGER NOT NULL DEFAULT 0, created_ms INTEGER NOT NULL, expires_ms INTEGER NOT NULL)",
            "CREATE TABLE IF NOT EXISTS log_sources (scan_id TEXT NOT NULL REFERENCES log_scans(id) ON DELETE CASCADE, source_id TEXT NOT NULL, source_json TEXT NOT NULL, native_position TEXT, integrity TEXT NOT NULL, reason TEXT, scanned_count INTEGER NOT NULL DEFAULT 0, matched_count INTEGER NOT NULL DEFAULT 0, at_tail INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(scan_id,source_id))",
            "CREATE TABLE IF NOT EXISTS log_events (seq INTEGER PRIMARY KEY AUTOINCREMENT, scan_id TEXT NOT NULL REFERENCES log_scans(id) ON DELETE CASCADE, source_id TEXT NOT NULL, native_position TEXT NOT NULL, event_ref TEXT NOT NULL UNIQUE, event_json TEXT NOT NULL, raw BLOB NOT NULL, UNIQUE(scan_id,source_id,native_position))",
            "CREATE INDEX IF NOT EXISTS log_events_scan_seq ON log_events(scan_id,seq)",
            "CREATE TABLE IF NOT EXISTS log_pages (cursor TEXT PRIMARY KEY, scan_id TEXT NOT NULL REFERENCES log_scans(id) ON DELETE CASCADE, after_seq INTEGER NOT NULL, page_json TEXT, next_cursor TEXT)",
        ] {
            sqlx::query(statement).execute(&pool).await?;
        }
        let columns = sqlx::query("PRAGMA table_info(log_scans)")
            .fetch_all(&pool)
            .await?;
        if !columns
            .iter()
            .any(|row| row.get::<String, _>("name") == "events_used")
        {
            sqlx::query("ALTER TABLE log_scans ADD COLUMN events_used INTEGER NOT NULL DEFAULT 0")
                .execute(&pool)
                .await?;
            sqlx::query("UPDATE log_scans SET events_used=(SELECT COUNT(*) FROM log_events WHERE log_events.scan_id=log_scans.id)")
                .execute(&pool).await?;
        }
        let source_columns = sqlx::query("PRAGMA table_info(log_sources)")
            .fetch_all(&pool)
            .await?;
        if !source_columns
            .iter()
            .any(|row| row.get::<String, _>("name") == "at_tail")
        {
            sqlx::query("ALTER TABLE log_sources ADD COLUMN at_tail INTEGER NOT NULL DEFAULT 0")
                .execute(&pool)
                .await?;
        }
        let store = Self { pool };
        store.cleanup_expired().await?;
        Ok(store)
    }

    pub async fn create_scan(&self, spec: NewScan<'_>) -> Result<(String, String), StoreError> {
        if spec.sources.is_empty() || spec.start_unix_ms >= spec.end_unix_ms {
            return Err(StoreError::Invalid(
                "sources and ordered time interval required",
            ));
        }
        let id = Uuid::new_v4().to_string();
        let cursor = Uuid::new_v4().to_string();
        let now = now_ms();
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM log_scans WHERE expires_ms<=?")
            .bind(now)
            .execute(&mut *tx)
            .await?;
        let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_scans")
            .fetch_one(&mut *tx)
            .await?;
        if active >= MAX_ACTIVE_SCANS {
            return Err(StoreError::Quota);
        }
        sqlx::query("INSERT INTO log_scans(id,actor_key,device_json,identity,start_ms,end_ms,mode,filter_json,created_ms,expires_ms) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(&id)
            .bind(spec.actor.storage_key())
            .bind(serde_json::to_string(&spec.device)?)
            .bind(spec.identity)
            .bind(spec.start_unix_ms)
            .bind(spec.end_unix_ms)
            .bind(serde_json::to_string(&spec.mode)?)
            .bind(spec.filter.map(serde_json::to_string).transpose()?)
            .bind(now)
            .bind(now.saturating_add(RETENTION_MS))
            .execute(&mut *tx).await?;
        for source in spec.sources {
            let (integrity, reason) = if source.availability == "readable" {
                (SystemLogIntegrity::Scanning, None)
            } else {
                (SystemLogIntegrity::Partial, source.reason.clone())
            };
            sqlx::query("INSERT INTO log_sources(scan_id,source_id,source_json,integrity,reason) VALUES(?,?,?,?,?)")
                .bind(&id)
                .bind(&source.source_id)
                .bind(serde_json::to_string(source)?)
                .bind(serde_json::to_string(&integrity)?)
                .bind(reason)
                .execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO log_pages(cursor,scan_id,after_seq) VALUES(?,?,0)")
            .bind(&cursor)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok((id, cursor))
    }

    /// The expected source position prevents two workers from accepting
    /// different batches for one source. Data and checkpoint are atomic.
    pub async fn append_page(
        &self,
        actor: OperatorRef,
        scan_id: &str,
        source_id: &str,
        expected_position: Option<&str>,
        page: &NativePage,
    ) -> Result<(), StoreError> {
        let mut connection = self.pool.acquire().await?;
        let mut tx = match sqlx::Transaction::<sqlx::Sqlite>::begin(
            &mut *connection,
            Some("BEGIN IMMEDIATE".into()),
        )
        .await
        {
            Ok(tx) => tx,
            Err(error) => {
                return Err(error.into());
            }
        };
        let result: Result<(), StoreError> = async {
        let scan = sqlx::query("SELECT actor_key,device_json,identity,filter_json,mode,bytes_used,events_used,expires_ms FROM log_scans WHERE id=?")
            .bind(scan_id).fetch_optional(&mut *tx).await?.ok_or(StoreError::NotFound)?;
        if scan.try_get::<String, _>("actor_key")? != actor.storage_key()
            || scan.try_get::<i64, _>("expires_ms")? <= now_ms()
        {
            return Err(StoreError::NotFound);
        }
        let source = sqlx::query(
            "SELECT native_position,integrity FROM log_sources WHERE scan_id=? AND source_id=?",
        )
        .bind(scan_id)
        .bind(source_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StoreError::NotFound)?;
        let saved_position: Option<String> = source.try_get("native_position")?;
        if saved_position.as_deref() != expected_position {
            return Err(StoreError::CheckpointConflict);
        }
        let integrity: String = source.try_get("integrity")?;
        if integrity != serde_json::to_string(&SystemLogIntegrity::Scanning)? {
            return Err(StoreError::CheckpointConflict);
        }
        let filter: Option<String> = scan.try_get("filter_json")?;
        let filter = filter
            .map(|value| serde_json::from_str::<SystemLogFilter>(&value))
            .transpose()?;
        let device: DeviceRef = serde_json::from_str(scan.try_get("device_json")?)?;
        let identity: String = scan.try_get("identity")?;
        let mut added_bytes = 0_i64;
        let mut matched = 0_i64;
        let total_used: i64 =
            sqlx::query_scalar("SELECT COALESCE(SUM(bytes_used),0) FROM log_scans")
                .fetch_one(&mut *tx)
                .await?;
        let total_events: i64 =
            sqlx::query_scalar("SELECT COALESCE(SUM(events_used),0) FROM log_scans")
                .fetch_one(&mut *tx)
                .await?;
        for native in &page.events {
            if native.source_id != source_id {
                return Err(StoreError::Invalid("native event source mismatch"));
            }
            if filter.as_ref().is_some_and(|filter| {
                filter
                    .native_level
                    .as_ref()
                    .is_some_and(|level| native.native_level.as_ref() != Some(level))
                    || filter.message_contains.as_ref().is_some_and(|needle| {
                        !native
                            .message
                            .as_ref()
                            .is_some_and(|value| value.contains(needle))
                    })
            }) {
                continue;
            }
            added_bytes = added_bytes.saturating_add(native.raw.len() as i64);
            if scan
                .try_get::<i64, _>("bytes_used")?
                .saturating_add(added_bytes)
                > MAX_SCAN_BYTES
            {
                return Err(StoreError::Quota);
            }
            if total_used.saturating_add(added_bytes) > MAX_TOTAL_BYTES {
                return Err(StoreError::Quota);
            }
            if scan
                .try_get::<i64, _>("events_used")?
                .saturating_add(matched)
                >= MAX_SCAN_EVENTS
                || total_events.saturating_add(matched) >= MAX_TOTAL_EVENTS
            {
                return Err(StoreError::Quota);
            }
            let event_ref = Uuid::new_v4().to_string();
            let sha256 = format!("{:x}", Sha256::digest(&native.raw));
            let preview = native.message.as_deref().unwrap_or("");
            let preview: String = preview.chars().take(1024).collect();
            let event = SystemLogEvent {
                device_ref: device,
                event_ref: event_ref.clone(),
                source_id: native.source_id.clone(),
                source_kind: native.source_kind.clone(),
                source_locator: native.source_locator.clone(),
                execution_identity: identity.clone(),
                native_position: Some(native.native_position.clone()),
                timestamp_unix_ms: native.timestamp_unix_ms,
                native_level: native.native_level.clone(),
                message_preview: preview,
                total_bytes: native.raw.len() as u64,
                sha256,
            };
            sqlx::query("INSERT INTO log_events(scan_id,source_id,native_position,event_ref,event_json,raw) VALUES(?,?,?,?,?,?)")
                .bind(scan_id).bind(source_id).bind(&native.native_position).bind(&event_ref)
                .bind(serde_json::to_string(&event)?).bind(&native.raw).execute(&mut *tx).await?;
            matched += 1;
        }
        #[cfg(test)]
        if std::env::var("PAB_LOG_TEST_EXIT_AFTER_INSERT").ok().as_deref() == Some(scan_id) {
            // The child test exits without unwinding while SQLite holds an
            // uncommitted transaction. This exercises actual process-death
            // recovery rather than only an ordinary returned SQL error.
            std::process::exit(137);
        }
        let next = page.next_native_position.as_deref().or(expected_position);
        let mode: SystemLogMode = serde_json::from_str(scan.try_get("mode")?)?;
        let new_integrity = if page.exhausted && mode == SystemLogMode::Range {
            if page.verifiable {
                SystemLogIntegrity::Complete
            } else {
                SystemLogIntegrity::Unverified
            }
        } else {
            SystemLogIntegrity::Scanning
        };
        sqlx::query("UPDATE log_sources SET native_position=?,integrity=?,scanned_count=scanned_count+?,matched_count=matched_count+?,at_tail=? WHERE scan_id=? AND source_id=?")
            .bind(next)
            .bind(serde_json::to_string(&new_integrity)?)
            .bind(page.scanned_count as i64)
            .bind(matched)
            .bind(i64::from(page.exhausted))
            .bind(scan_id).bind(source_id).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE log_scans SET bytes_used=bytes_used+?,events_used=events_used+? WHERE id=?",
        )
        .bind(added_bytes)
        .bind(matched)
        .bind(scan_id)
        .execute(&mut *tx)
        .await?;
        Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                let committed = tx.commit().await;
                if committed.is_err() {
                    connection.close_on_drop();
                }
                committed?;
                Ok(())
            }
            Err(error) => {
                let rolled_back = tx.rollback().await;
                if rolled_back.is_err() || matches!(error, StoreError::Sql(_)) {
                    connection.close_on_drop();
                }
                Err(error)
            }
        }
    }

    pub async fn mark_source(
        &self,
        actor: OperatorRef,
        scan_id: &str,
        source_id: &str,
        integrity: SystemLogIntegrity,
        reason: &str,
    ) -> Result<(), StoreError> {
        self.require_owner(actor, scan_id).await?;
        let changed = sqlx::query(
            "UPDATE log_sources SET integrity=?,reason=? WHERE scan_id=? AND source_id=?",
        )
        .bind(serde_json::to_string(&integrity)?)
        .bind(reason)
        .bind(scan_id)
        .bind(source_id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed == 0 {
            Err(StoreError::NotFound)
        } else {
            Ok(())
        }
    }

    async fn require_owner(&self, actor: OperatorRef, scan_id: &str) -> Result<(), StoreError> {
        let owner: Option<(String, i64)> =
            sqlx::query_as("SELECT actor_key,expires_ms FROM log_scans WHERE id=?")
                .bind(scan_id)
                .fetch_optional(&self.pool)
                .await?;
        match owner {
            Some((key, expires)) if key == actor.storage_key() && expires > now_ms() => Ok(()),
            _ => Err(StoreError::NotFound),
        }
    }

    pub async fn read_page(
        &self,
        actor: OperatorRef,
        cursor: &str,
        limit: u16,
    ) -> Result<SystemLogData, StoreError> {
        if !(1..=128).contains(&limit) {
            return Err(StoreError::Invalid("page limit 1..128"));
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row = sqlx::query("SELECT p.scan_id,p.after_seq,p.page_json,s.actor_key,s.expires_ms,s.mode FROM log_pages p JOIN log_scans s ON s.id=p.scan_id WHERE p.cursor=?")
            .bind(cursor).fetch_optional(&mut *tx).await?.ok_or(StoreError::NotFound)?;
        if row.try_get::<String, _>("actor_key")? != actor.storage_key()
            || row.try_get::<i64, _>("expires_ms")? <= now_ms()
        {
            return Err(StoreError::NotFound);
        }
        let scan_id: String = row.try_get("scan_id")?;
        sqlx::query("UPDATE log_scans SET expires_ms=? WHERE id=?")
            .bind(now_ms().saturating_add(RETENTION_MS))
            .bind(&scan_id)
            .execute(&mut *tx)
            .await?;
        if let Some(saved) = row.try_get::<Option<String>, _>("page_json")? {
            tx.commit().await?;
            return Ok(serde_json::from_str(&saved)?);
        }
        let after_seq: i64 = row.try_get("after_seq")?;
        let rows = sqlx::query(
            "SELECT seq,event_json FROM log_events WHERE scan_id=? AND seq>? ORDER BY seq LIMIT ?",
        )
        .bind(&scan_id)
        .bind(after_seq)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await?;
        let mut events = Vec::new();
        let mut last_seq = after_seq;
        let mut budget = 0_usize;
        for event_row in rows {
            let serialized: String = event_row.try_get("event_json")?;
            if !events.is_empty() && budget + serialized.len() > PAGE_BUDGET {
                break;
            }
            budget += serialized.len();
            last_seq = event_row.try_get("seq")?;
            events.push(serde_json::from_str::<SystemLogEvent>(&serialized)?);
        }
        let progress_rows = sqlx::query("SELECT source_id,native_position,integrity,reason,scanned_count,matched_count FROM log_sources WHERE scan_id=? ORDER BY source_id LIMIT ?")
            .bind(&scan_id).bind(SOURCE_PROGRESS_LIMIT).fetch_all(&mut *tx).await?;
        let mut sources = Vec::new();
        for progress in progress_rows {
            sources.push(SystemLogSourceProgress {
                source_id: progress.try_get("source_id")?,
                native_position: progress.try_get("native_position")?,
                integrity: serde_json::from_str(progress.try_get("integrity")?)?,
                reason: progress.try_get("reason")?,
                scanned_count: progress.try_get::<i64, _>("scanned_count")? as u64,
                matched_count: progress.try_get::<i64, _>("matched_count")? as u64,
            });
        }
        let source_total: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM log_sources WHERE scan_id=?")
                .bind(&scan_id)
                .fetch_one(&mut *tx)
                .await?;
        let mut aggregate = SystemLogIntegrity::Complete;
        let all_progress = sqlx::query("SELECT integrity FROM log_sources WHERE scan_id=?")
            .bind(&scan_id)
            .fetch_all(&mut *tx)
            .await?;
        for progress in all_progress {
            let item: SystemLogIntegrity = serde_json::from_str(progress.try_get("integrity")?)?;
            aggregate = combine(aggregate, item);
        }
        let latest_seq: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(seq),0) FROM log_events WHERE scan_id=?")
                .bind(&scan_id)
                .fetch_one(&mut *tx)
                .await?;
        let mode: SystemLogMode = serde_json::from_str(row.try_get("mode")?)?;
        // The age covers only committed events beyond this cursor. A native
        // source may still have unscanned records; report that separately as
        // catching_up without inventing an age for records not yet observed.
        let oldest_unread_age_ms = if mode == SystemLogMode::Follow {
            let pending: Option<String> = sqlx::query_scalar(
                "SELECT event_json FROM log_events WHERE scan_id=? AND seq>? ORDER BY seq LIMIT 1",
            )
            .bind(&scan_id)
            .bind(last_seq)
            .fetch_optional(&mut *tx)
            .await?;
            pending
                .map(|json| serde_json::from_str::<SystemLogEvent>(&json))
                .transpose()?
                .map(|event| now_ms().saturating_sub(event.timestamp_unix_ms).max(0) as u64)
        } else {
            None
        };
        let capture_state = if mode == SystemLogMode::Follow {
            let behind: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM log_sources WHERE scan_id=? AND integrity=? AND at_tail=0",
            )
            .bind(&scan_id)
            .bind(serde_json::to_string(&SystemLogIntegrity::Scanning)?)
            .fetch_one(&mut *tx)
            .await?;
            Some(
                if matches!(
                    aggregate,
                    SystemLogIntegrity::Gap
                        | SystemLogIntegrity::Partial
                        | SystemLogIntegrity::Unverified
                ) {
                    "stopped"
                } else if oldest_unread_age_ms.is_some_and(|age| age >= LAGGING_AGE_MS) {
                    "lagging"
                } else if behind > 0 || latest_seq > last_seq {
                    "catching_up"
                } else {
                    "following"
                }
                .into(),
            )
        } else {
            None
        };
        let has_more = latest_seq > last_seq
            || aggregate == SystemLogIntegrity::Scanning
            || capture_state.is_some();
        let empty_poll = events.is_empty()
            && latest_seq == after_seq
            && (mode == SystemLogMode::Follow || aggregate == SystemLogIntegrity::Scanning);
        let next_cursor = if empty_poll {
            Some(cursor.to_owned())
        } else {
            has_more.then(|| Uuid::new_v4().to_string())
        };
        let data = SystemLogData::Page {
            events,
            sources,
            source_total: source_total as u32,
            source_progress_truncated: source_total > SOURCE_PROGRESS_LIMIT,
            integrity: aggregate,
            capture_state,
            cursor: next_cursor.clone(),
            has_more,
            oldest_unread_age_ms,
        };
        // A poll with no matched event keeps the same cursor. Caching a new
        // empty page for every scan round or follow poll would grow SQLite
        // without a bound during quiet or filtered sessions.
        if empty_poll {
            tx.commit().await?;
            return Ok(data);
        }
        let serialized = serde_json::to_string(&data)?;
        if serialized.len() > 30 * 1024 {
            return Err(StoreError::Invalid(
                "log page metadata exceeds reply budget",
            ));
        }
        sqlx::query("UPDATE log_pages SET page_json=?,next_cursor=? WHERE cursor=?")
            .bind(serialized)
            .bind(&next_cursor)
            .bind(cursor)
            .execute(&mut *tx)
            .await?;
        if let Some(next) = next_cursor {
            sqlx::query("INSERT INTO log_pages(cursor,scan_id,after_seq) VALUES(?,?,?)")
                .bind(next)
                .bind(&scan_id)
                .bind(last_seq)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(data)
    }

    pub async fn read_event(
        &self,
        actor: OperatorRef,
        event_ref: &str,
        offset: u64,
        max_bytes: u32,
    ) -> Result<SystemLogData, StoreError> {
        if !(1..=8192).contains(&max_bytes) {
            return Err(StoreError::Invalid("event byte limit 1..8192"));
        }
        let row = sqlx::query("SELECT e.raw,e.event_json,s.actor_key,s.expires_ms FROM log_events e JOIN log_scans s ON s.id=e.scan_id WHERE e.event_ref=?")
            .bind(event_ref).fetch_optional(&self.pool).await?.ok_or(StoreError::NotFound)?;
        if row.try_get::<String, _>("actor_key")? != actor.storage_key()
            || row.try_get::<i64, _>("expires_ms")? <= now_ms()
        {
            return Err(StoreError::NotFound);
        }
        let raw: Vec<u8> = row.try_get("raw")?;
        let event: SystemLogEvent = serde_json::from_str(row.try_get("event_json")?)?;
        let start =
            usize::try_from(offset).map_err(|_| StoreError::Invalid("event offset too large"))?;
        if start > raw.len() {
            return Err(StoreError::Invalid("event offset past end"));
        }
        let end = raw.len().min(start.saturating_add(max_bytes as usize));
        Ok(SystemLogData::EventChunk {
            event_ref: event_ref.into(),
            offset,
            next_offset: end as u64,
            total_bytes: raw.len() as u64,
            data_base64: base64::engine::general_purpose::STANDARD.encode(&raw[start..end]),
            sha256: event.sha256,
        })
    }

    pub async fn scan_spec(
        &self,
        actor: OperatorRef,
        scan_id: &str,
    ) -> Result<(i64, i64, SystemLogMode, Option<SystemLogFilter>), StoreError> {
        self.require_owner(actor, scan_id).await?;
        let row = sqlx::query("SELECT start_ms,end_ms,mode,filter_json FROM log_scans WHERE id=?")
            .bind(scan_id)
            .fetch_one(&self.pool)
            .await?;
        let filter: Option<String> = row.try_get("filter_json")?;
        Ok((
            row.try_get("start_ms")?,
            row.try_get("end_ms")?,
            serde_json::from_str(row.try_get("mode")?)?,
            filter.map(|f| serde_json::from_str(&f)).transpose()?,
        ))
    }

    pub async fn scan_sources(
        &self,
        actor: OperatorRef,
        scan_id: &str,
    ) -> Result<Vec<(SystemLogSource, Option<String>, SystemLogIntegrity)>, StoreError> {
        self.require_owner(actor, scan_id).await?;
        let rows = sqlx::query("SELECT source_json,native_position,integrity FROM log_sources WHERE scan_id=? ORDER BY source_id")
            .bind(scan_id).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    serde_json::from_str(row.try_get("source_json")?)?,
                    row.try_get("native_position")?,
                    serde_json::from_str(row.try_get("integrity")?)?,
                ))
            })
            .collect()
    }

    pub async fn scan_id_for_cursor(
        &self,
        actor: OperatorRef,
        cursor: &str,
    ) -> Result<String, StoreError> {
        let row = sqlx::query("SELECT p.scan_id,s.actor_key,s.expires_ms FROM log_pages p JOIN log_scans s ON s.id=p.scan_id WHERE p.cursor=?")
            .bind(cursor).fetch_optional(&self.pool).await?.ok_or(StoreError::NotFound)?;
        if row.try_get::<String, _>("actor_key")? != actor.storage_key()
            || row.try_get::<i64, _>("expires_ms")? <= now_ms()
        {
            return Err(StoreError::NotFound);
        }
        Ok(row.try_get("scan_id")?)
    }

    pub async fn source_progress_page(
        &self,
        actor: OperatorRef,
        cursor: &str,
        after: Option<&str>,
        limit: u16,
    ) -> Result<
        (
            Vec<SystemLogSource>,
            Vec<SystemLogSourceProgress>,
            Option<String>,
            u32,
        ),
        StoreError,
    > {
        if !(1..=128).contains(&limit) {
            return Err(StoreError::Invalid("source limit 1..128"));
        }
        let scan_id = self.scan_id_for_cursor(actor, cursor).await?;
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_sources WHERE scan_id=?")
            .bind(&scan_id)
            .fetch_one(&self.pool)
            .await?;
        let rows = sqlx::query("SELECT source_id,source_json,native_position,integrity,reason,scanned_count,matched_count FROM log_sources WHERE scan_id=? AND source_id>? ORDER BY source_id LIMIT ?")
            .bind(&scan_id).bind(after.unwrap_or("")).bind(i64::from(limit)+1).fetch_all(&self.pool).await?;
        let available = rows.len();
        let mut sources = Vec::new();
        let mut progress = Vec::new();
        let mut used = 0_usize;
        for row in rows.into_iter().take(usize::from(limit)) {
            let serialized: String = row.try_get("source_json")?;
            let item = SystemLogSourceProgress {
                source_id: row.try_get("source_id")?,
                native_position: row.try_get("native_position")?,
                integrity: serde_json::from_str(row.try_get("integrity")?)?,
                reason: row.try_get("reason")?,
                scanned_count: row.try_get::<i64, _>("scanned_count")? as u64,
                matched_count: row.try_get::<i64, _>("matched_count")? as u64,
            };
            let size = serialized.len() + serde_json::to_vec(&item)?.len();
            if !sources.is_empty() && used + size > 20 * 1024 {
                break;
            }
            used += size;
            sources.push(serde_json::from_str::<SystemLogSource>(&serialized)?);
            progress.push(item);
        }
        let has_more = available > sources.len();
        let next = if has_more {
            sources.last().map(|s| s.source_id.clone())
        } else {
            None
        };
        Ok((sources, progress, next, total as u32))
    }

    pub async fn cleanup_expired(&self) -> Result<u64, StoreError> {
        Ok(sqlx::query("DELETE FROM log_scans WHERE expires_ms<=?")
            .bind(now_ms())
            .execute(&self.pool)
            .await?
            .rows_affected())
    }
}

fn combine(a: SystemLogIntegrity, b: SystemLogIntegrity) -> SystemLogIntegrity {
    use SystemLogIntegrity::*;
    match (a, b) {
        (Gap, _) | (_, Gap) => Gap,
        (Partial, _) | (_, Partial) => Partial,
        (Unverified, _) | (_, Unverified) => Unverified,
        (Scanning, _) | (_, Scanning) => Scanning,
        _ => Complete,
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use pab_os_system_log::NativeRecord;
    use pab_protocol::{DeviceId, EndpointKey, TenantId};

    fn actor(byte: u8) -> OperatorRef {
        OperatorRef::guest(EndpointKey::new([byte; 32]))
    }

    fn device() -> DeviceRef {
        DeviceRef {
            tenant_id: TenantId::from_u128(1),
            device_id: DeviceId::from_u128(2),
        }
    }

    fn source() -> SystemLogSource {
        SystemLogSource {
            source_id: "synthetic:system".into(),
            source_kind: "synthetic".into(),
            source_locator: "test".into(),
            availability: "readable".into(),
            reason: None,
            execution_identity: "test".into(),
            coverage_limit: None,
        }
    }

    fn page(start: usize, count: usize, exhausted: bool) -> NativePage {
        NativePage {
            events: (start..start + count)
                .map(|n| NativeRecord {
                    source_id: "synthetic:system".into(),
                    source_kind: "synthetic".into(),
                    source_locator: "test".into(),
                    native_position: n.to_string(),
                    timestamp_unix_ms: n as i64,
                    native_level: Some("info".into()),
                    raw: format!("entry-{n}").into_bytes(),
                    message: Some(format!("entry-{n}")),
                })
                .collect(),
            next_native_position: (count > 0).then(|| (start + count - 1).to_string()),
            scanned_count: count as u64,
            exhausted,
            verifiable: true,
        }
    }

    #[tokio::test]
    async fn empty_follow_polls_reuse_cursor_without_storing_pages() {
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::open(&dir.path().join("logs.db")).await.unwrap();
        let owner = actor(1);
        let sources = [source()];
        let (scan, cursor) = store
            .create_scan(NewScan {
                actor: owner,
                device: device(),
                identity: "test",
                sources: &sources,
                start_unix_ms: 0,
                end_unix_ms: i64::MAX,
                mode: SystemLogMode::Follow,
                filter: None,
            })
            .await
            .unwrap();
        for _ in 0..25 {
            let SystemLogData::Page {
                events,
                cursor: returned,
                has_more,
                ..
            } = store.read_page(owner, &cursor, 1).await.unwrap()
            else {
                panic!("page");
            };
            assert!(events.is_empty() && has_more);
            assert_eq!(returned.as_deref(), Some(cursor.as_str()));
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_pages WHERE scan_id=?")
            .bind(&scan)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        store
            .append_page(owner, &scan, "synthetic:system", None, &page(1, 1, false))
            .await
            .unwrap();
        let SystemLogData::Page {
            events,
            cursor: Some(next),
            ..
        } = store.read_page(owner, &cursor, 1).await.unwrap()
        else {
            panic!("event page");
        };
        assert_eq!(events.len(), 1);
        assert_ne!(next, cursor);
    }

    #[tokio::test]
    async fn follow_distinguishes_native_catchup_from_aged_unread_events() {
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::open(&dir.path().join("logs.db")).await.unwrap();
        let owner = actor(9);
        let (scan, cursor) = store
            .create_scan(NewScan {
                actor: owner,
                device: device(),
                identity: "test",
                sources: &[source()],
                start_unix_ms: 0,
                end_unix_ms: i64::MAX,
                mode: SystemLogMode::Follow,
                filter: None,
            })
            .await
            .unwrap();
        let SystemLogData::Page {
            capture_state,
            oldest_unread_age_ms,
            ..
        } = store.read_page(owner, &cursor, 1).await.unwrap()
        else {
            panic!("initial follow page")
        };
        assert_eq!(capture_state.as_deref(), Some("catching_up"));
        assert_eq!(oldest_unread_age_ms, None);

        let mut batch = page(1, 2, false);
        for event in &mut batch.events {
            event.timestamp_unix_ms = now_ms() - 10_000;
        }
        store
            .append_page(owner, &scan, "synthetic:system", None, &batch)
            .await
            .unwrap();
        let first = store.read_page(owner, &cursor, 1).await.unwrap();
        let SystemLogData::Page {
            events,
            cursor: Some(next),
            capture_state,
            oldest_unread_age_ms,
            ..
        } = &first
        else {
            panic!("first follow event")
        };
        assert_eq!(events.len(), 1);
        assert_eq!(capture_state.as_deref(), Some("lagging"));
        assert!(oldest_unread_age_ms.unwrap() >= 9_000);
        assert_eq!(store.read_page(owner, &cursor, 1).await.unwrap(), first);

        let SystemLogData::Page {
            events,
            cursor: Some(after_second),
            capture_state,
            oldest_unread_age_ms,
            ..
        } = store.read_page(owner, next, 1).await.unwrap()
        else {
            panic!("second follow event")
        };
        assert_eq!(events.len(), 1);
        assert_eq!(capture_state.as_deref(), Some("catching_up"));
        assert_eq!(oldest_unread_age_ms, None);

        store
            .append_page(
                owner,
                &scan,
                "synthetic:system",
                Some("2"),
                &page(3, 0, true),
            )
            .await
            .unwrap();
        let SystemLogData::Page {
            events,
            capture_state,
            oldest_unread_age_ms,
            ..
        } = store.read_page(owner, &after_second, 1).await.unwrap()
        else {
            panic!("caught-up follow page")
        };
        assert!(events.is_empty());
        assert_eq!(capture_state.as_deref(), Some("following"));
        assert_eq!(oldest_unread_age_ms, None);

        store
            .mark_source(
                owner,
                &scan,
                "synthetic:system",
                SystemLogIntegrity::Gap,
                "injected loss",
            )
            .await
            .unwrap();
        let SystemLogData::Page {
            capture_state,
            integrity,
            ..
        } = store.read_page(owner, &after_second, 1).await.unwrap()
        else {
            panic!("stopped follow page")
        };
        assert_eq!(integrity, SystemLogIntegrity::Gap);
        assert_eq!(capture_state.as_deref(), Some("stopped"));
    }

    #[tokio::test]
    async fn follow_does_not_claim_live_with_unread_committed_events() {
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::open(&dir.path().join("logs.db")).await.unwrap();
        let owner = actor(9);
        let (scan, cursor) = store
            .create_scan(NewScan {
                actor: owner,
                device: device(),
                identity: "test",
                sources: &[source()],
                start_unix_ms: 0,
                end_unix_ms: i64::MAX,
                mode: SystemLogMode::Follow,
                filter: None,
            })
            .await
            .unwrap();
        let mut batch = page(1, 2, true);
        for event in &mut batch.events {
            event.timestamp_unix_ms = now_ms();
        }
        store
            .append_page(owner, &scan, "synthetic:system", None, &batch)
            .await
            .unwrap();
        let SystemLogData::Page {
            cursor: Some(next),
            capture_state,
            oldest_unread_age_ms,
            ..
        } = store.read_page(owner, &cursor, 1).await.unwrap()
        else {
            panic!("first follow page")
        };
        assert_eq!(capture_state.as_deref(), Some("catching_up"));
        assert!(oldest_unread_age_ms.is_some());
        let SystemLogData::Page { capture_state, .. } =
            store.read_page(owner, &next, 1).await.unwrap()
        else {
            panic!("second follow page")
        };
        assert_eq!(capture_state.as_deref(), Some("following"));
    }

    #[tokio::test]
    async fn existing_log_source_schema_adds_tail_state_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs.db");
        let old = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::query("CREATE TABLE log_sources (scan_id TEXT NOT NULL, source_id TEXT NOT NULL, source_json TEXT NOT NULL, native_position TEXT, integrity TEXT NOT NULL, reason TEXT, scanned_count INTEGER NOT NULL DEFAULT 0, matched_count INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(scan_id,source_id))")
            .execute(&old)
            .await
            .unwrap();
        old.close().await;
        let store = LogStore::open(&path).await.unwrap();
        let columns = sqlx::query("PRAGMA table_info(log_sources)")
            .fetch_all(&store.pool)
            .await
            .unwrap();
        assert!(columns
            .iter()
            .any(|column| column.get::<String, _>("name") == "at_tail"));
    }

    #[tokio::test]
    async fn event_quota_rejects_batch_without_advancing_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::open(&dir.path().join("logs.db")).await.unwrap();
        let owner = actor(1);
        let sources = [source()];
        let (scan, _) = store
            .create_scan(NewScan {
                actor: owner,
                device: device(),
                identity: "test",
                sources: &sources,
                start_unix_ms: 0,
                end_unix_ms: 100,
                mode: SystemLogMode::Range,
                filter: None,
            })
            .await
            .unwrap();
        sqlx::query("UPDATE log_scans SET events_used=? WHERE id=?")
            .bind(MAX_SCAN_EVENTS)
            .bind(&scan)
            .execute(&store.pool)
            .await
            .unwrap();
        assert!(matches!(
            store
                .append_page(owner, &scan, "synthetic:system", None, &page(0, 1, false))
                .await,
            Err(StoreError::Quota)
        ));
        let (source, position, state) = store.scan_sources(owner, &scan).await.unwrap().remove(0);
        assert_eq!(source.source_id, "synthetic:system");
        assert_eq!(position, None);
        assert_eq!(state, SystemLogIntegrity::Scanning);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_events WHERE scan_id=?")
            .bind(&scan)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn checkpoint_write_failure_rolls_back_events_and_can_retry() {
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::open(&dir.path().join("logs.db")).await.unwrap();
        let owner = actor(1);
        let sources = [source()];
        let (scan, _) = store
            .create_scan(NewScan {
                actor: owner,
                device: device(),
                identity: "test",
                sources: &sources,
                start_unix_ms: 0,
                end_unix_ms: 100,
                mode: SystemLogMode::Range,
                filter: None,
            })
            .await
            .unwrap();
        sqlx::query(
            "CREATE TRIGGER fail_log_checkpoint BEFORE UPDATE OF native_position ON log_sources
             BEGIN SELECT RAISE(ABORT, 'injected checkpoint failure'); END",
        )
        .execute(&store.pool)
        .await
        .unwrap();
        assert!(store
            .append_page(owner, &scan, "synthetic:system", None, &page(0, 1, false))
            .await
            .is_err());
        sqlx::query("DROP TRIGGER fail_log_checkpoint")
            .execute(&store.pool)
            .await
            .unwrap();
        let (_, position, state) = store.scan_sources(owner, &scan).await.unwrap().remove(0);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_events WHERE scan_id=?")
            .bind(&scan)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(position, None);
        assert_eq!(state, SystemLogIntegrity::Scanning);
        assert_eq!(count, 0);
        store
            .append_page(owner, &scan, "synthetic:system", None, &page(0, 1, false))
            .await
            .unwrap();
        let (_, position, _) = store.scan_sources(owner, &scan).await.unwrap().remove(0);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_events WHERE scan_id=?")
            .bind(&scan)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(position.as_deref(), Some("0"));
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn log_store_hard_exit_child() {
        let Ok(path) = std::env::var("PAB_LOG_TEST_CHILD_DB") else {
            return;
        };
        let scan = std::env::var("PAB_LOG_TEST_EXIT_AFTER_INSERT").unwrap();
        let store = LogStore::open(Path::new(&path)).await.unwrap();
        store
            .append_page(
                actor(1),
                &scan,
                "synthetic:system",
                None,
                &page(0, 1, false),
            )
            .await
            .unwrap();
        panic!("hard-exit injection was not reached");
    }

    #[tokio::test]
    async fn process_exit_after_event_insert_recovers_without_advancing_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs.db");
        let store = LogStore::open(&path).await.unwrap();
        let owner = actor(1);
        let sources = [source()];
        let (scan, _) = store
            .create_scan(NewScan {
                actor: owner,
                device: device(),
                identity: "test",
                sources: &sources,
                start_unix_ms: 0,
                end_unix_ms: 100,
                mode: SystemLogMode::Range,
                filter: None,
            })
            .await
            .unwrap();
        drop(store);
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::log_store_hard_exit_child", "--nocapture"])
            .env("PAB_LOG_TEST_CHILD_DB", &path)
            .env("PAB_LOG_TEST_EXIT_AFTER_INSERT", &scan)
            .status()
            .unwrap();
        assert_eq!(child.code(), Some(137));
        let store = LogStore::open(&path).await.unwrap();
        let (_, position, state) = store.scan_sources(owner, &scan).await.unwrap().remove(0);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_events WHERE scan_id=?")
            .bind(&scan)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(position, None);
        assert_eq!(state, SystemLogIntegrity::Scanning);
        assert_eq!(count, 0);
        store
            .append_page(owner, &scan, "synthetic:system", None, &page(0, 1, false))
            .await
            .unwrap();
        let (_, position, _) = store.scan_sources(owner, &scan).await.unwrap().remove(0);
        assert_eq!(position.as_deref(), Some("0"));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn real_tmpfs_exhaustion_preserves_event_and_checkpoint_atomicity() {
        let Ok(folder) = std::env::var("PAB_LOG_TEST_TMPFS") else {
            return;
        };
        // This test deliberately fills a filesystem. Restrict it to the
        // disposable Docker tmpfs created by the acceptance command.
        assert_eq!(folder, "/pab-log-test");
        let path = Path::new(&folder);
        let store = LogStore::open(&path.join("logs.db")).await.unwrap();
        let owner = actor(1);
        let sources = [source()];
        let (scan, _) = store
            .create_scan(NewScan {
                actor: owner,
                device: device(),
                identity: "test",
                sources: &sources,
                start_unix_ms: 0,
                end_unix_ms: 100,
                mode: SystemLogMode::Range,
                filter: None,
            })
            .await
            .unwrap();
        let c_path = std::ffi::CString::new(folder.as_str()).unwrap();
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        assert_eq!(
            unsafe { libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) },
            0
        );
        let stats = unsafe { stats.assume_init() };
        let free = (stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64);
        assert!(
            (1_000_000..=16_000_000).contains(&free),
            "unexpected tmpfs size: {free}"
        );
        let filler = path.join("filler");
        let mut output = std::fs::File::create(&filler).unwrap();
        let mut remaining = free.saturating_sub(256 * 1024);
        let chunk = [0_u8; 64 * 1024];
        while remaining > 0 {
            use std::io::Write as _;
            let count = remaining.min(chunk.len() as u64) as usize;
            output.write_all(&chunk[..count]).unwrap();
            remaining -= count as u64;
        }
        output.sync_all().unwrap();
        drop(output);
        let mut large = page(0, 1, false);
        large.events[0].raw = vec![b'X'; 1024 * 1024];
        let failure = store
            .append_page(owner, &scan, "synthetic:system", None, &large)
            .await
            .unwrap_err();
        assert!(
            matches!(&failure, StoreError::Sql(sqlx::Error::Database(error))
                if error.code().as_deref() == Some("13")),
            "expected SQLITE_FULL, got {failure:?}"
        );
        std::fs::remove_file(&filler).unwrap();
        let (_, position, state) = store.scan_sources(owner, &scan).await.unwrap().remove(0);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_events WHERE scan_id=?")
            .bind(&scan)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(position, None);
        assert_eq!(state, SystemLogIntegrity::Scanning);
        assert_eq!(count, 0);
        store
            .append_page(owner, &scan, "synthetic:system", None, &page(0, 1, false))
            .await
            .unwrap();
        let (_, position, _) = store.scan_sources(owner, &scan).await.unwrap().remove(0);
        assert_eq!(position.as_deref(), Some("0"));
    }

    #[tokio::test]
    async fn checkpoint_page_and_full_event_survive_reopen_without_duplication() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs.db");
        let store = LogStore::open(&path).await.unwrap();
        let owner = actor(1);
        let sources = [source()];
        let (scan, cursor) = store
            .create_scan(NewScan {
                actor: owner,
                device: device(),
                identity: "test",
                sources: &sources,
                start_unix_ms: 0,
                end_unix_ms: 1000,
                mode: SystemLogMode::Range,
                filter: None,
            })
            .await
            .unwrap();
        store
            .append_page(owner, &scan, "synthetic:system", None, &page(0, 3, false))
            .await
            .unwrap();
        assert!(matches!(
            store
                .append_page(owner, &scan, "synthetic:system", None, &page(0, 3, false))
                .await,
            Err(StoreError::CheckpointConflict)
        ));
        drop(store);
        let store = LogStore::open(&path).await.unwrap();
        store
            .append_page(
                owner,
                &scan,
                "synthetic:system",
                Some("2"),
                &page(3, 2, true),
            )
            .await
            .unwrap();
        let first = store.read_page(owner, &cursor, 2).await.unwrap();
        assert_eq!(first, store.read_page(owner, &cursor, 2).await.unwrap());
        let SystemLogData::Page {
            events,
            cursor: Some(next),
            has_more,
            ..
        } = first
        else {
            panic!("first page")
        };
        assert!(has_more);
        assert_eq!(
            events
                .iter()
                .map(|e| e.native_position.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("0"), Some("1")]
        );
        let first_ref = events[0].event_ref.clone();
        let SystemLogData::Page {
            events,
            cursor: Some(last),
            ..
        } = store.read_page(owner, &next, 2).await.unwrap()
        else {
            panic!("second page")
        };
        assert_eq!(
            events
                .iter()
                .map(|e| e.native_position.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("2"), Some("3")]
        );
        let SystemLogData::Page {
            events,
            integrity,
            has_more,
            ..
        } = store.read_page(owner, &last, 2).await.unwrap()
        else {
            panic!("last page")
        };
        assert_eq!(events[0].native_position.as_deref(), Some("4"));
        assert_eq!(integrity, SystemLogIntegrity::Complete);
        assert!(!has_more);
        assert!(matches!(
            store.read_page(actor(2), &cursor, 2).await,
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.read_event(actor(2), &first_ref, 0, 16).await,
            Err(StoreError::NotFound)
        ));
        let SystemLogData::EventChunk {
            data_base64,
            sha256,
            ..
        } = store.read_event(owner, &first_ref, 0, 16).await.unwrap()
        else {
            panic!("event chunk")
        };
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(data_base64)
                .unwrap(),
            b"entry-0"
        );
        assert_eq!(sha256, format!("{:x}", Sha256::digest(b"entry-0")));
    }

    #[tokio::test]
    async fn unavailable_source_and_filtered_zero_match_are_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::open(&dir.path().join("logs.db")).await.unwrap();
        let owner = actor(3);
        let mut denied = source();
        denied.source_id = "synthetic:denied".into();
        denied.availability = "permission_denied".into();
        denied.reason = Some("access denied".into());
        let (scan, cursor) = store
            .create_scan(NewScan {
                actor: owner,
                device: device(),
                identity: "test",
                sources: &[source(), denied],
                start_unix_ms: 0,
                end_unix_ms: 1000,
                mode: SystemLogMode::Range,
                filter: Some(&SystemLogFilter {
                    message_contains: Some("missing".into()),
                    native_level: None,
                }),
            })
            .await
            .unwrap();
        store
            .append_page(owner, &scan, "synthetic:system", None, &page(0, 3, true))
            .await
            .unwrap();
        let SystemLogData::Page {
            events,
            sources,
            integrity,
            has_more,
            ..
        } = store.read_page(owner, &cursor, 10).await.unwrap()
        else {
            panic!("page")
        };
        assert!(events.is_empty());
        assert_eq!(integrity, SystemLogIntegrity::Partial);
        assert!(!has_more);
        assert_eq!(
            sources
                .iter()
                .find(|s| s.source_id == "synthetic:system")
                .unwrap()
                .scanned_count,
            3
        );
    }

    #[tokio::test]
    async fn independent_sequence_manifest_matches_every_page_size_without_loss() {
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::open(&dir.path().join("logs.db")).await.unwrap();
        let owner = actor(5);
        let source = [source()];
        let expected = (0..257).map(|n| n.to_string()).collect::<Vec<_>>();
        for limit in [1, 2, 7, 128] {
            let (scan, mut cursor) = store
                .create_scan(NewScan {
                    actor: owner,
                    device: device(),
                    identity: "test",
                    sources: &source,
                    start_unix_ms: 0,
                    end_unix_ms: 1000,
                    mode: SystemLogMode::Range,
                    filter: None,
                })
                .await
                .unwrap();
            let mut after = None;
            for start in (0..257).step_by(31) {
                let count = (257 - start).min(31);
                store
                    .append_page(
                        owner,
                        &scan,
                        "synthetic:system",
                        after.as_deref(),
                        &page(start, count, start + count == 257),
                    )
                    .await
                    .unwrap();
                after = Some((start + count - 1).to_string());
            }
            let mut actual = Vec::new();
            loop {
                let SystemLogData::Page {
                    events,
                    cursor: next,
                    has_more,
                    integrity,
                    ..
                } = store.read_page(owner, &cursor, limit).await.unwrap()
                else {
                    panic!("page")
                };
                assert_eq!(integrity, SystemLogIntegrity::Complete);
                actual.extend(
                    events
                        .iter()
                        .map(|event| event.native_position.clone().unwrap()),
                );
                if !has_more {
                    break;
                }
                cursor = next.unwrap();
            }
            assert_eq!(actual, expected, "page limit {limit}");
        }
    }

    #[tokio::test]
    async fn empty_verifiable_range_is_complete_and_retry_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::open(&dir.path().join("logs.db")).await.unwrap();
        let owner = actor(7);
        let (scan, cursor) = store
            .create_scan(NewScan {
                actor: owner,
                device: device(),
                identity: "test",
                sources: &[source()],
                start_unix_ms: 0,
                end_unix_ms: 100,
                mode: SystemLogMode::Range,
                filter: None,
            })
            .await
            .unwrap();
        store
            .append_page(owner, &scan, "synthetic:system", None, &page(0, 0, true))
            .await
            .unwrap();
        let result = store.read_page(owner, &cursor, 1).await.unwrap();
        assert_eq!(result, store.read_page(owner, &cursor, 1).await.unwrap());
        let SystemLogData::Page {
            events,
            sources,
            integrity,
            has_more,
            ..
        } = result
        else {
            panic!("empty range page")
        };
        assert!(events.is_empty());
        assert!(!has_more);
        assert_eq!(integrity, SystemLogIntegrity::Complete);
        assert_eq!(sources[0].scanned_count, 0);
        assert_eq!(sources[0].matched_count, 0);
    }

    #[tokio::test]
    async fn interleaved_sources_preserve_equal_timestamps_and_large_binary_event() {
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::open(&dir.path().join("logs.db")).await.unwrap();
        let owner = actor(8);
        let mut other = source();
        other.source_id = "synthetic:other".into();
        let (scan, mut cursor) = store
            .create_scan(NewScan {
                actor: owner,
                device: device(),
                identity: "test",
                sources: &[source(), other],
                start_unix_ms: 0,
                end_unix_ms: 100,
                mode: SystemLogMode::Range,
                filter: None,
            })
            .await
            .unwrap();
        let mut expected_raw = vec![0xff; 70_001];
        expected_raw.extend_from_slice("日志✓".as_bytes());
        for (source_id, start) in [("synthetic:system", 0), ("synthetic:other", 3)] {
            let mut batch = page(start, 3, true);
            for event in &mut batch.events {
                event.source_id = source_id.into();
                event.timestamp_unix_ms = 42;
                event.message = Some(format!("日志✓ {}", event.native_position));
            }
            if start == 0 {
                batch.events[0].raw = expected_raw.clone();
            }
            store
                .append_page(owner, &scan, source_id, None, &batch)
                .await
                .unwrap();
        }
        let mut actual = Vec::new();
        let mut large_ref = None;
        loop {
            let SystemLogData::Page {
                events,
                sources,
                integrity,
                cursor: next,
                has_more,
                ..
            } = store.read_page(owner, &cursor, 2).await.unwrap()
            else {
                panic!("interleaved page")
            };
            assert_eq!(integrity, SystemLogIntegrity::Complete);
            assert_eq!(sources.len(), 2);
            for event in events {
                assert_eq!(event.timestamp_unix_ms, 42);
                assert!(event.message_preview.starts_with("日志✓"));
                if event.source_id == "synthetic:system"
                    && event.native_position.as_deref() == Some("0")
                {
                    large_ref = Some(event.event_ref.clone());
                    assert_eq!(event.total_bytes, expected_raw.len() as u64);
                }
                actual.push((event.source_id, event.native_position.unwrap()));
            }
            if !has_more {
                break;
            }
            cursor = next.unwrap();
        }
        assert_eq!(
            actual,
            [
                ("synthetic:system".into(), "0".into()),
                ("synthetic:system".into(), "1".into()),
                ("synthetic:system".into(), "2".into()),
                ("synthetic:other".into(), "3".into()),
                ("synthetic:other".into(), "4".into()),
                ("synthetic:other".into(), "5".into()),
            ]
        );
        let event_ref = large_ref.unwrap();
        let mut reconstructed = Vec::new();
        while reconstructed.len() < expected_raw.len() {
            let SystemLogData::EventChunk {
                data_base64,
                next_offset,
                sha256,
                ..
            } = store
                .read_event(owner, &event_ref, reconstructed.len() as u64, 8192)
                .await
                .unwrap()
            else {
                panic!("large event chunk")
            };
            reconstructed.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(data_base64)
                    .unwrap(),
            );
            assert_eq!(next_offset, reconstructed.len() as u64);
            assert_eq!(sha256, format!("{:x}", Sha256::digest(&expected_raw)));
        }
        assert_eq!(reconstructed, expected_raw);
    }

    #[tokio::test]
    async fn explicit_gap_never_becomes_complete() {
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::open(&dir.path().join("logs.db")).await.unwrap();
        let owner = actor(6);
        let (scan, cursor) = store
            .create_scan(NewScan {
                actor: owner,
                device: device(),
                identity: "test",
                sources: &[source()],
                start_unix_ms: 0,
                end_unix_ms: 1000,
                mode: SystemLogMode::Range,
                filter: None,
            })
            .await
            .unwrap();
        store
            .append_page(owner, &scan, "synthetic:system", None, &page(0, 2, false))
            .await
            .unwrap();
        store
            .mark_source(
                owner,
                &scan,
                "synthetic:system",
                SystemLogIntegrity::Gap,
                "native cursor removed",
            )
            .await
            .unwrap();
        let SystemLogData::Page {
            integrity,
            sources,
            has_more,
            ..
        } = store.read_page(owner, &cursor, 32).await.unwrap()
        else {
            panic!("page")
        };
        assert_eq!(integrity, SystemLogIntegrity::Gap);
        assert!(!has_more);
        assert_eq!(sources[0].reason.as_deref(), Some("native cursor removed"));
    }
}
