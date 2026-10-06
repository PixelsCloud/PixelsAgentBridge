use std::path::Path;
use std::time::Duration;

use pab_protocol::{
    CommandTaskSpec, DeviceRef, OutputChunk, OutputRange, OutputStream, RequestId, TaskEvent,
    TaskId, TaskRef, TaskSnapshot,
};
use sqlx::{
    Row, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use thiserror::Error;

const MAX_RETAINED_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTaskRecord {
    pub request_id: RequestId,
    pub device_ref: DeviceRef,
    pub command: Option<CommandTaskSpec>,
    pub snapshot: Option<TaskSnapshot>,
    pub last_event_seq: u64,
    pub stdout: OutputRange,
    pub stderr: OutputRange,
}

impl LocalTaskRecord {
    pub fn is_complete(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.state.is_terminal()
                && self.last_event_seq == snapshot.latest_event_seq
                && output_is_complete(&self.stdout, &snapshot.output.stdout)
                && output_is_complete(&self.stderr, &snapshot.output.stderr)
        })
    }
}

#[derive(Clone)]
pub(super) struct RuntimeStore {
    pub(super) pool: SqlitePool,
}

pub(super) struct OutputGap {
    pub stream: OutputStream,
    pub missing_from: u64,
    pub missing_to: u64,
}

impl RuntimeStore {
    pub async fn open(path: &Path) -> Result<Self, RuntimeStoreError> {
        let mut attempt = 0;
        loop {
            match Self::open_once(path).await {
                Err(RuntimeStoreError::Database(sqlx::Error::Database(error)))
                    if matches!(error.code().as_deref(), Some("5" | "6")) && attempt < 19 =>
                {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                result => return result,
            }
        }
    }

    async fn open_once(path: &Path) -> Result<Self, RuntimeStoreError> {
        pab_agent_core::ensure_data_parent(path).map_err(RuntimeStoreError::Io)?;
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        for statement in [
            "CREATE TABLE IF NOT EXISTS runtime_screenshot_results (id TEXT PRIMARY KEY, meta_json TEXT NOT NULL, FOREIGN KEY(id) REFERENCES runtime_operations(id) ON DELETE CASCADE)",
            "CREATE TABLE IF NOT EXISTS runtime_system_results (id TEXT PRIMARY KEY, query_json TEXT NOT NULL, reply_json TEXT NOT NULL, FOREIGN KEY(id) REFERENCES runtime_operations(id) ON DELETE CASCADE)",
            r#"
                CREATE TABLE IF NOT EXISTS runtime_tasks (
                    request_id TEXT PRIMARY KEY,
                    device_ref_json TEXT NOT NULL,
                    command_json TEXT NOT NULL,
                    task_id TEXT UNIQUE,
                    snapshot_json TEXT,
                    last_event_seq INTEGER NOT NULL DEFAULT 0,
                    stdout_retained_from INTEGER NOT NULL DEFAULT 0,
                    stdout_available_to INTEGER NOT NULL DEFAULT 0,
                    stdout_complete INTEGER NOT NULL DEFAULT 0,
                    stdout_bytes BLOB NOT NULL DEFAULT X'',
                    stderr_retained_from INTEGER NOT NULL DEFAULT 0,
                    stderr_available_to INTEGER NOT NULL DEFAULT 0,
                    stderr_complete INTEGER NOT NULL DEFAULT 0,
                    stderr_bytes BLOB NOT NULL DEFAULT X''
                )
                "#,
            r#"
                CREATE TABLE IF NOT EXISTS runtime_task_events (
                    task_id TEXT NOT NULL,
                    seq INTEGER NOT NULL,
                    event_json TEXT NOT NULL,
                    PRIMARY KEY (task_id, seq),
                    FOREIGN KEY (task_id) REFERENCES runtime_tasks(task_id) ON DELETE CASCADE
                )
                "#,
            "CREATE INDEX IF NOT EXISTS runtime_tasks_device ON runtime_tasks (device_ref_json)",
            r#"
                CREATE TABLE IF NOT EXISTS remembered_devices (
                    device_ref_json TEXT PRIMARY KEY,
                    device_code TEXT NOT NULL UNIQUE,
                    alias TEXT NOT NULL DEFAULT '',
                    os_family_json TEXT NOT NULL,
                    os_reminder TEXT NOT NULL
                )
                "#,
            r#"
                CREATE TABLE IF NOT EXISTS device_credentials (
                    device_id TEXT PRIMARY KEY,
                    password TEXT NOT NULL
                )
                "#,
            r#"
                CREATE TABLE IF NOT EXISTS runtime_operations (
                    id TEXT PRIMARY KEY,
                    device_ref_json TEXT NOT NULL,
                    device_code TEXT,
                    initiated_by TEXT NOT NULL DEFAULT 'unknown',
                    kind TEXT NOT NULL,
                    direction TEXT NOT NULL,
                    source TEXT NOT NULL,
                    destination TEXT NOT NULL,
                    overwrite INTEGER NOT NULL,
                    state TEXT NOT NULL,
                    offset INTEGER NOT NULL DEFAULT 0,
                    size INTEGER NOT NULL DEFAULT 0,
                    started_at_unix_ms INTEGER NOT NULL,
                    finished_at_unix_ms INTEGER,
                    message TEXT,
                    owner_session_id TEXT
                )
                "#,
            "CREATE INDEX IF NOT EXISTS runtime_operations_started ON runtime_operations (started_at_unix_ms DESC)",
            "CREATE INDEX IF NOT EXISTS runtime_operations_owner ON runtime_operations (owner_session_id)",
            "CREATE TABLE IF NOT EXISTS runtime_async_transfers (id TEXT PRIMARY KEY REFERENCES runtime_operations(id), phase TEXT NOT NULL, updated_at_unix_ms INTEGER NOT NULL, sha256 TEXT)",
            "CREATE TABLE IF NOT EXISTS runtime_transfer_context (id TEXT PRIMARY KEY REFERENCES runtime_operations(id), options_json TEXT NOT NULL, context_json TEXT)",
            "CREATE TABLE IF NOT EXISTS runtime_terminal_identity (id TEXT PRIMARY KEY REFERENCES runtime_operations(id) ON DELETE CASCADE, identity_json TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS runtime_task_owners (request_id TEXT PRIMARY KEY REFERENCES runtime_tasks(request_id), owner_session_id TEXT NOT NULL, initiated_by TEXT NOT NULL, created_at_unix_ms INTEGER NOT NULL)",
            "CREATE INDEX IF NOT EXISTS runtime_task_owners_session ON runtime_task_owners (owner_session_id, created_at_unix_ms DESC)",
            "CREATE TABLE IF NOT EXISTS runtime_download_claims (destination_key TEXT PRIMARY KEY, operation_id TEXT NOT NULL REFERENCES runtime_operations(id))",
            "CREATE TABLE IF NOT EXISTS runtime_filesystem_results (id TEXT PRIMARY KEY REFERENCES runtime_operations(id), fingerprint TEXT NOT NULL, reply_json TEXT NOT NULL)",
            r#"
                CREATE TABLE IF NOT EXISTS runtime_sessions (
                    id TEXT PRIMARY KEY,
                    heartbeat_at_unix_ms INTEGER NOT NULL,
                    stopped_at_unix_ms INTEGER
                )
                "#,
            r#"
                CREATE TABLE IF NOT EXISTS runtime_terminal_events (
                    session_id TEXT NOT NULL,
                    seq INTEGER NOT NULL,
                    kind TEXT NOT NULL,
                    payload BLOB NOT NULL,
                    state TEXT NOT NULL,
                    at_unix_ms INTEGER NOT NULL,
                    PRIMARY KEY (session_id, seq),
                    FOREIGN KEY (session_id) REFERENCES runtime_operations(id)
                )
                "#,
        ] {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        // Project observed results only. A requested username or context_ref
        // must never appear as the identity that actually executed an operation.
        sqlx::query("CREATE VIEW IF NOT EXISTS runtime_operation_identity AS SELECT o.id, CASE WHEN o.kind = 'terminal' THEN t.identity_json WHEN o.kind = 'file_transfer' THEN json_extract(c.context_json, '$.identity') WHEN f.id IS NOT NULL THEN json_extract(f.reply_json, '$.execution_context.identity') ELSE json_extract(q.reply_json, '$.execution_context.identity') END AS identity_json FROM runtime_operations o LEFT JOIN runtime_terminal_identity t ON t.id=o.id LEFT JOIN runtime_transfer_context c ON c.id=o.id LEFT JOIN runtime_filesystem_results f ON f.id=o.id LEFT JOIN runtime_system_results q ON q.id=o.id")
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Self { pool })
    }

    #[cfg(test)]
    pub(super) async fn close(self) {
        self.pool.close().await;
    }

    pub async fn record_pending(
        &self,
        device_ref: DeviceRef,
        request_id: RequestId,
        command: &CommandTaskSpec,
    ) -> Result<LocalTaskRecord, RuntimeStoreError> {
        // Reserve the writer before reading. A deferred WAL transaction can
        // fail its read-to-write upgrade immediately when another MCP commits,
        // even with busy_timeout configured. Use IMMEDIATE for write transactions.
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(row) = sqlx::query("SELECT * FROM runtime_tasks WHERE request_id = ?")
            .bind(request_id.to_string())
            .fetch_optional(&mut *tx)
            .await?
        {
            let record = decode_record(&row)?;
            if record.device_ref != device_ref || record.command.as_ref() != Some(command) {
                return Err(RuntimeStoreError::RequestConflict);
            }
            tx.commit().await?;
            return Ok(record);
        }
        sqlx::query(
            "INSERT INTO runtime_tasks (request_id, device_ref_json, command_json) VALUES (?, ?, ?)",
        )
        .bind(request_id.to_string())
        .bind(serde_json::to_string(&device_ref)?)
        .bind(serde_json::to_string(command)?)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.get_by_request(request_id).await
    }

    pub async fn bind_snapshot(
        &self,
        snapshot: &TaskSnapshot,
    ) -> Result<LocalTaskRecord, RuntimeStoreError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row = sqlx::query("SELECT * FROM runtime_tasks WHERE request_id = ?")
            .bind(snapshot.request_id.to_string())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(RuntimeStoreError::NotFound)?;
        let record = decode_record(&row)?;
        if record.device_ref != snapshot.task_ref.device_ref
            || record
                .snapshot
                .as_ref()
                .is_some_and(|stored| stored.task_ref != snapshot.task_ref)
        {
            return Err(RuntimeStoreError::SnapshotIdentityMismatch);
        }
        sqlx::query("UPDATE runtime_tasks SET task_id = ?, snapshot_json = ? WHERE request_id = ?")
            .bind(snapshot.task_ref.task_id.to_string())
            .bind(serde_json::to_string(snapshot)?)
            .bind(snapshot.request_id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.get_by_request(snapshot.request_id).await
    }

    pub async fn adopt_snapshot(
        &self,
        snapshot: &TaskSnapshot,
    ) -> Result<LocalTaskRecord, RuntimeStoreError> {
        if let Ok(record) = self.get_by_task(snapshot.task_ref).await {
            if record.request_id != snapshot.request_id {
                return Err(RuntimeStoreError::SnapshotIdentityMismatch);
            }
            return self.update_snapshot(snapshot).await;
        }
        let stdout_start = snapshot.output.stdout.retained_from;
        let stderr_start = snapshot.output.stderr.retained_from;
        sqlx::query(
            r#"
            INSERT INTO runtime_tasks (
                request_id, device_ref_json, command_json, task_id, snapshot_json,
                stdout_retained_from, stdout_available_to, stdout_complete,
                stderr_retained_from, stderr_available_to, stderr_complete
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(snapshot.request_id.to_string())
        .bind(serde_json::to_string(&snapshot.task_ref.device_ref)?)
        .bind("null")
        .bind(snapshot.task_ref.task_id.to_string())
        .bind(serde_json::to_string(snapshot)?)
        .bind(to_i64(stdout_start)?)
        .bind(to_i64(stdout_start)?)
        .bind(
            snapshot.output.stdout.complete && stdout_start == snapshot.output.stdout.available_to,
        )
        .bind(to_i64(stderr_start)?)
        .bind(to_i64(stderr_start)?)
        .bind(
            snapshot.output.stderr.complete && stderr_start == snapshot.output.stderr.available_to,
        )
        .execute(&self.pool)
        .await?;
        self.get_by_task(snapshot.task_ref).await
    }

    pub async fn record_event(&self, event: &TaskEvent) -> Result<bool, RuntimeStoreError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row = row_by_task(&mut tx, event.task_ref).await?;
        let record = decode_record(&row)?;
        if event.seq <= record.last_event_seq {
            tx.commit().await?;
            return Ok(false);
        }
        let expected = record
            .last_event_seq
            .checked_add(1)
            .ok_or(RuntimeStoreError::OffsetTooLarge)?;
        if event.seq != expected {
            return Err(RuntimeStoreError::EventGap {
                expected,
                actual: event.seq,
            });
        }
        let seq = to_i64(event.seq)?;
        sqlx::query("INSERT INTO runtime_task_events (task_id, seq, event_json) VALUES (?, ?, ?)")
            .bind(event.task_ref.task_id.to_string())
            .bind(seq)
            .bind(serde_json::to_string(event)?)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE runtime_tasks SET last_event_seq = ? WHERE task_id = ?")
            .bind(seq)
            .bind(event.task_ref.task_id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn append_output(
        &self,
        chunk: &OutputChunk,
        range: &OutputRange,
    ) -> Result<bool, RuntimeStoreError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row = row_by_task(&mut tx, chunk.task_ref).await?;
        let record = decode_record(&row)?;
        let local = match chunk.stream {
            OutputStream::Stdout => &record.stdout,
            OutputStream::Stderr => &record.stderr,
        };
        if chunk.offset < local.available_to {
            tx.commit().await?;
            return Ok(false);
        }
        if chunk.offset != local.available_to {
            return Err(RuntimeStoreError::OutputGap {
                expected: local.available_to,
                actual: chunk.offset,
            });
        }
        let added =
            u64::try_from(chunk.bytes.len()).map_err(|_| RuntimeStoreError::OffsetTooLarge)?;
        let available_to = chunk
            .offset
            .checked_add(added)
            .ok_or(RuntimeStoreError::OffsetTooLarge)?;
        if range.retained_from > chunk.offset || available_to > range.available_to {
            return Err(RuntimeStoreError::InvalidOutputRange);
        }
        let retained_length = local
            .available_to
            .checked_sub(local.retained_from)
            .ok_or(RuntimeStoreError::InvalidOutputRange)?;
        let combined_length = retained_length
            .checked_add(added)
            .ok_or(RuntimeStoreError::OffsetTooLarge)?;
        let trimmed = combined_length.saturating_sub(MAX_RETAINED_OUTPUT_BYTES);
        let retained_from = local
            .retained_from
            .checked_add(trimmed)
            .ok_or(RuntimeStoreError::OffsetTooLarge)?;
        let (prefix, bytes_column) = stream_columns(chunk.stream);
        let query = format!(
            "UPDATE runtime_tasks SET {prefix}_retained_from = ?, {prefix}_available_to = ?, {prefix}_bytes = substr(CAST({bytes_column} || ? AS BLOB), ?) WHERE task_id = ?"
        );
        sqlx::query(&query)
            .bind(to_i64(retained_from)?)
            .bind(to_i64(available_to)?)
            .bind(&chunk.bytes)
            .bind(to_i64(trimmed.saturating_add(1))?)
            .bind(chunk.task_ref.task_id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(!chunk.bytes.is_empty())
    }

    pub async fn update_snapshot(
        &self,
        snapshot: &TaskSnapshot,
    ) -> Result<LocalTaskRecord, RuntimeStoreError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row = row_by_task(&mut tx, snapshot.task_ref).await?;
        let record = decode_record(&row)?;
        if record.request_id != snapshot.request_id {
            return Err(RuntimeStoreError::SnapshotIdentityMismatch);
        }
        let stdout_complete = record.stdout.available_to == snapshot.output.stdout.available_to
            && snapshot.output.stdout.complete;
        let stderr_complete = record.stderr.available_to == snapshot.output.stderr.available_to
            && snapshot.output.stderr.complete;
        sqlx::query(
            "UPDATE runtime_tasks SET snapshot_json = ?, stdout_complete = ?, stderr_complete = ? WHERE task_id = ?",
        )
        .bind(serde_json::to_string(snapshot)?)
        .bind(stdout_complete)
        .bind(stderr_complete)
        .bind(snapshot.task_ref.task_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.get_by_request(snapshot.request_id).await
    }

    pub async fn reconcile_snapshot(
        &self,
        snapshot: &TaskSnapshot,
    ) -> Result<(LocalTaskRecord, Vec<OutputGap>), RuntimeStoreError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row = row_by_task(&mut tx, snapshot.task_ref).await?;
        let record = decode_record(&row)?;
        if record.request_id != snapshot.request_id {
            return Err(RuntimeStoreError::SnapshotIdentityMismatch);
        }
        let mut gaps = Vec::new();
        for (stream, local, remote) in [
            (
                OutputStream::Stdout,
                &record.stdout,
                &snapshot.output.stdout,
            ),
            (
                OutputStream::Stderr,
                &record.stderr,
                &snapshot.output.stderr,
            ),
        ] {
            if local.available_to > remote.available_to {
                return Err(RuntimeStoreError::InvalidOutputRange);
            }
            let (prefix, bytes_column) = stream_columns(stream);
            if local.available_to < remote.retained_from {
                gaps.push(OutputGap {
                    stream,
                    missing_from: local.available_to,
                    missing_to: remote.retained_from,
                });
                let query = format!(
                    "UPDATE runtime_tasks SET {prefix}_retained_from = ?, {prefix}_available_to = ?, {prefix}_complete = ?, {bytes_column} = X'' WHERE task_id = ?"
                );
                sqlx::query(&query)
                    .bind(to_i64(remote.retained_from)?)
                    .bind(to_i64(remote.retained_from)?)
                    .bind(remote.complete && remote.retained_from == remote.available_to)
                    .bind(snapshot.task_ref.task_id.to_string())
                    .execute(&mut *tx)
                    .await?;
            } else {
                let complete = remote.complete && local.available_to == remote.available_to;
                let query =
                    format!("UPDATE runtime_tasks SET {prefix}_complete = ? WHERE task_id = ?");
                sqlx::query(&query)
                    .bind(complete)
                    .bind(snapshot.task_ref.task_id.to_string())
                    .execute(&mut *tx)
                    .await?;
            }
        }
        sqlx::query("UPDATE runtime_tasks SET snapshot_json = ? WHERE task_id = ?")
            .bind(serde_json::to_string(snapshot)?)
            .bind(snapshot.task_ref.task_id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok((self.get_by_request(snapshot.request_id).await?, gaps))
    }

    pub async fn get_by_request(
        &self,
        request_id: RequestId,
    ) -> Result<LocalTaskRecord, RuntimeStoreError> {
        let row = sqlx::query("SELECT * FROM runtime_tasks WHERE request_id = ?")
            .bind(request_id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or(RuntimeStoreError::NotFound)?;
        decode_record(&row)
    }

    pub async fn get_by_task(
        &self,
        task_ref: TaskRef,
    ) -> Result<LocalTaskRecord, RuntimeStoreError> {
        let row = sqlx::query("SELECT * FROM runtime_tasks WHERE task_id = ?")
            .bind(task_ref.task_id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or(RuntimeStoreError::NotFound)?;
        let record = decode_record(&row)?;
        if record.device_ref != task_ref.device_ref {
            return Err(RuntimeStoreError::NotFound);
        }
        Ok(record)
    }

    pub async fn get_by_task_id(
        &self,
        task_id: TaskId,
    ) -> Result<LocalTaskRecord, RuntimeStoreError> {
        let row = sqlx::query("SELECT * FROM runtime_tasks WHERE task_id = ?")
            .bind(task_id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or(RuntimeStoreError::NotFound)?;
        decode_record(&row)
    }

    pub async fn list(&self) -> Result<Vec<LocalTaskRecord>, RuntimeStoreError> {
        let rows = sqlx::query("SELECT * FROM runtime_tasks ORDER BY rowid DESC")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(decode_record).collect()
    }

    pub async fn list_page(
        &self,
        before: Option<RequestId>,
        limit: u32,
    ) -> Result<Vec<LocalTaskRecord>, RuntimeStoreError> {
        let rows = sqlx::query(
            "SELECT * FROM runtime_tasks WHERE snapshot_json IS NOT NULL AND (? IS NULL OR rowid < (SELECT rowid FROM runtime_tasks WHERE request_id = ?)) ORDER BY rowid DESC LIMIT ?",
        )
        .bind(before.map(|value| value.to_string()))
        .bind(before.map(|value| value.to_string()))
        .bind(i64::from(limit.min(101)))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(decode_record).collect()
    }

    pub async fn list_page_for_device(
        &self,
        device_ref: DeviceRef,
        before: Option<RequestId>,
        limit: u32,
    ) -> Result<Vec<LocalTaskRecord>, RuntimeStoreError> {
        let rows = sqlx::query(
            "SELECT * FROM runtime_tasks WHERE device_ref_json = ? AND snapshot_json IS NOT NULL AND (? IS NULL OR rowid < (SELECT rowid FROM runtime_tasks WHERE request_id = ?)) ORDER BY rowid DESC LIMIT ?",
        )
        .bind(serde_json::to_string(&device_ref)?)
        .bind(before.map(|value| value.to_string()))
        .bind(before.map(|value| value.to_string()))
        .bind(i64::from(limit.min(101)))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(decode_record).collect()
    }

    pub async fn incomplete(&self) -> Result<Vec<LocalTaskRecord>, RuntimeStoreError> {
        Ok(self
            .list()
            .await?
            .into_iter()
            .filter(|record| !record.is_complete())
            .collect())
    }

    pub async fn events_after(
        &self,
        task_ref: TaskRef,
        after_seq: u64,
    ) -> Result<Vec<TaskEvent>, RuntimeStoreError> {
        self.get_by_task(task_ref).await?;
        let rows = sqlx::query(
            "SELECT event_json FROM runtime_task_events WHERE task_id = ? AND seq > ? ORDER BY seq",
        )
        .bind(task_ref.task_id.to_string())
        .bind(to_i64(after_seq)?)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| serde_json::from_str(row.get("event_json")).map_err(Into::into))
            .collect()
    }

    pub async fn read_output(
        &self,
        task_ref: TaskRef,
        stream: OutputStream,
        offset: u64,
        max_bytes: u32,
    ) -> Result<(OutputChunk, OutputRange), RuntimeStoreError> {
        // Range and bytes must come from the same WAL snapshot while other
        // MCP processes append or trim their cached output.
        let mut tx = self.pool.begin().await?;
        let record = decode_record(&row_by_task(&mut tx, task_ref).await?)?;
        let range = match stream {
            OutputStream::Stdout => record.stdout,
            OutputStream::Stderr => record.stderr,
        };
        if offset < range.retained_from || offset > range.available_to {
            return Err(RuntimeStoreError::InvalidOutputOffset);
        }
        let relative = offset
            .checked_sub(range.retained_from)
            .and_then(|value| value.checked_add(1))
            .ok_or(RuntimeStoreError::OffsetTooLarge)?;
        let (_, bytes_column) = stream_columns(stream);
        let query = format!(
            "SELECT substr({bytes_column}, ?, ?) AS chunk FROM runtime_tasks WHERE task_id = ?"
        );
        let row = sqlx::query(&query)
            .bind(to_i64(relative)?)
            .bind(i64::from(max_bytes))
            .bind(task_ref.task_id.to_string())
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok((
            OutputChunk {
                schema_version: pab_protocol::TASK_SCHEMA_VERSION,
                task_ref,
                stream,
                offset,
                bytes: row.get("chunk"),
            },
            range,
        ))
    }
}

async fn row_by_task<'a>(
    tx: &mut Transaction<'a, sqlx::Sqlite>,
    task_ref: TaskRef,
) -> Result<sqlx::sqlite::SqliteRow, RuntimeStoreError> {
    let row = sqlx::query("SELECT * FROM runtime_tasks WHERE task_id = ?")
        .bind(task_ref.task_id.to_string())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(RuntimeStoreError::NotFound)?;
    let device_ref: DeviceRef = serde_json::from_str(row.get("device_ref_json"))?;
    if device_ref != task_ref.device_ref {
        return Err(RuntimeStoreError::NotFound);
    }
    Ok(row)
}

fn decode_record(row: &sqlx::sqlite::SqliteRow) -> Result<LocalTaskRecord, RuntimeStoreError> {
    let snapshot_json: Option<&str> = row.get("snapshot_json");
    Ok(LocalTaskRecord {
        request_id: row
            .get::<&str, _>("request_id")
            .parse()
            .map_err(|_| RuntimeStoreError::InvalidStoredIdentifier)?,
        device_ref: serde_json::from_str(row.get("device_ref_json"))?,
        command: serde_json::from_str(row.get("command_json"))?,
        snapshot: snapshot_json.map(serde_json::from_str).transpose()?,
        last_event_seq: from_i64(row.get("last_event_seq"))?,
        stdout: OutputRange {
            retained_from: from_i64(row.get("stdout_retained_from"))?,
            available_to: from_i64(row.get("stdout_available_to"))?,
            complete: row.get("stdout_complete"),
        },
        stderr: OutputRange {
            retained_from: from_i64(row.get("stderr_retained_from"))?,
            available_to: from_i64(row.get("stderr_available_to"))?,
            complete: row.get("stderr_complete"),
        },
    })
}

fn output_is_complete(local: &OutputRange, remote: &OutputRange) -> bool {
    local.complete && local.available_to == remote.available_to
}

fn stream_columns(stream: OutputStream) -> (&'static str, &'static str) {
    match stream {
        OutputStream::Stdout => ("stdout", "stdout_bytes"),
        OutputStream::Stderr => ("stderr", "stderr_bytes"),
    }
}

fn to_i64(value: u64) -> Result<i64, RuntimeStoreError> {
    i64::try_from(value).map_err(|_| RuntimeStoreError::OffsetTooLarge)
}

fn from_i64(value: i64) -> Result<u64, RuntimeStoreError> {
    u64::try_from(value).map_err(|_| RuntimeStoreError::InvalidStoredOffset)
}

#[derive(Debug, Error)]
pub enum RuntimeStoreError {
    #[error("Bridge Runtime data directory could not be created: {0}")]
    Io(std::io::Error),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("the request ID already exists with a different device or command")]
    RequestConflict,
    #[error(
        "this MCP has 32 unresolved filesystem operations; resolve them before submitting more"
    )]
    FilesystemBusy,
    #[error("this MCP has 16 unresolved system queries; query existing IDs first")]
    SystemQueryBusy,
    #[error("the local task record was not found")]
    NotFound,
    #[error("the remote task snapshot does not match the local request")]
    SnapshotIdentityMismatch,
    #[error("stored identifier is invalid")]
    InvalidStoredIdentifier,
    #[error("stored output offset is invalid")]
    InvalidStoredOffset,
    #[error("task event stream has a gap: expected {expected}, received {actual}")]
    EventGap { expected: u64, actual: u64 },
    #[error("task output has a gap: expected offset {expected}, received {actual}")]
    OutputGap { expected: u64, actual: u64 },
    #[error("Executor returned an invalid output range")]
    InvalidOutputRange,
    #[error("task output offset is outside the locally retained range")]
    InvalidOutputOffset,
    #[error("task event or output offset is too large")]
    OffsetTooLarge,
}

impl RuntimeStoreError {
    pub const fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound)
    }
}
