use std::path::Path;

use pab_protocol::{
    CapabilityRef, CommandTaskSpec, DeviceRef, OperatorRef, OutputChunk, OutputRange, OutputStream,
    RequestId, TASK_SCHEMA_VERSION, TaskEvent, TaskEventKind, TaskId, TaskRef, TaskSnapshot,
};
use pab_task_runtime::{AcceptedTask, TaskAggregate, TaskRuntimeError};
use sqlx::{
    Row, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use thiserror::Error;
use tokio::sync::broadcast;

const CHANGE_BUFFER: usize = 1024;
const MAX_RETAINED_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

mod directory;
mod filesystem;
mod operation;
mod system_query;
mod terminal;
mod transfer_execution;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskChangeKind {
    Event,
    Output(OutputStream),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TaskChange {
    pub task_ref: TaskRef,
    pub kind: TaskChangeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AcceptTaskOutcome {
    Created(TaskSnapshot),
    Existing(TaskSnapshot),
}

#[derive(Clone)]
pub(crate) struct TaskStore {
    pool: SqlitePool,
    changes: broadcast::Sender<TaskChange>,
}

impl TaskStore {
    pub async fn open(path: &Path) -> Result<Self, TaskStoreError> {
        pab_agent_core::ensure_data_parent(path).map_err(TaskStoreError::Io)?;
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let mut tx = pool.begin().await?;
        for statement in [
            "CREATE TABLE IF NOT EXISTS system_query_results (request_id TEXT PRIMARY KEY, query_json TEXT NOT NULL, reply_json TEXT NOT NULL, FOREIGN KEY(request_id) REFERENCES read_operations(request_id) ON DELETE CASCADE)",
            r#"
            CREATE TABLE IF NOT EXISTS task_records (
                task_id TEXT PRIMARY KEY,
                initiated_by TEXT NOT NULL,
                request_id TEXT NOT NULL,
                snapshot_json TEXT NOT NULL,
                command_json TEXT NOT NULL,
                UNIQUE (initiated_by, request_id)
            )"#,
            r#"
            CREATE TABLE IF NOT EXISTS task_events (
                task_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                event_json TEXT NOT NULL,
                PRIMARY KEY (task_id, seq),
                FOREIGN KEY (task_id) REFERENCES task_records(task_id) ON DELETE CASCADE
            )"#,
            r#"
            CREATE TABLE IF NOT EXISTS task_outputs (
                task_id TEXT NOT NULL,
                stream TEXT NOT NULL,
                bytes BLOB NOT NULL,
                PRIMARY KEY (task_id, stream),
                FOREIGN KEY (task_id) REFERENCES task_records(task_id) ON DELETE CASCADE
            )"#,
            r#"
            CREATE TABLE IF NOT EXISTS transfer_operations (
                request_id TEXT PRIMARY KEY,
                initiated_by_json TEXT NOT NULL,
                direction TEXT NOT NULL,
                path TEXT NOT NULL,
                size INTEGER NOT NULL DEFAULT 0,
                offset INTEGER NOT NULL DEFAULT 0,
                state TEXT NOT NULL,
                started_at_unix_ms INTEGER NOT NULL,
                finished_at_unix_ms INTEGER,
                message TEXT,
                sha256 TEXT
            )"#,
            r#"
            CREATE TABLE IF NOT EXISTS read_operations (
                request_id TEXT PRIMARY KEY,
                initiated_by_json TEXT NOT NULL,
                kind TEXT NOT NULL,
                path TEXT NOT NULL,
                state TEXT NOT NULL,
                result_count INTEGER NOT NULL DEFAULT 0,
                started_at_unix_ms INTEGER NOT NULL,
                finished_at_unix_ms INTEGER,
                message TEXT
            )"#,
            r#"
            CREATE TABLE IF NOT EXISTS terminal_sessions (
                id TEXT PRIMARY KEY,
                initiated_by_json TEXT NOT NULL,
                shell TEXT NOT NULL,
                state TEXT NOT NULL,
                started_at_unix_ms INTEGER NOT NULL,
                finished_at_unix_ms INTEGER,
                message TEXT
            )"#,
            r#"
            CREATE TABLE IF NOT EXISTS terminal_events (
                session_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                kind TEXT NOT NULL,
                payload BLOB NOT NULL,
                state TEXT NOT NULL,
                at_unix_ms INTEGER NOT NULL,
                PRIMARY KEY (session_id, seq),
                FOREIGN KEY (session_id) REFERENCES terminal_sessions(id)
            )"#,
        ] {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        sqlx::query("CREATE TABLE IF NOT EXISTS filesystem_results (request_id TEXT PRIMARY KEY REFERENCES read_operations(request_id), fingerprint TEXT NOT NULL, reply_json TEXT)").execute(&mut *tx).await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS terminal_execution (session_id TEXT PRIMARY KEY REFERENCES terminal_sessions(id), connection_id TEXT NOT NULL, selection_json TEXT NOT NULL, identity_json TEXT, cols INTEGER NOT NULL, rows INTEGER NOT NULL)").execute(&mut *tx).await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS transfer_execution (request_id TEXT PRIMARY KEY REFERENCES transfer_operations(request_id), fingerprint TEXT NOT NULL, request_json TEXT NOT NULL, context_json TEXT NOT NULL)").execute(&mut *tx).await?;
        tx.commit().await?;
        let (changes, _) = broadcast::channel(CHANGE_BUFFER);
        Ok(Self { pool, changes })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TaskChange> {
        self.changes.subscribe()
    }

    pub async fn incomplete_task_refs(&self) -> Result<Vec<TaskRef>, TaskStoreError> {
        let rows = sqlx::query("SELECT snapshot_json FROM task_records")
            .fetch_all(&self.pool)
            .await?;
        let mut task_refs = Vec::new();
        for row in rows {
            let snapshot = decode_snapshot(row.get("snapshot_json"))?;
            if !snapshot.state.is_terminal() {
                task_refs.push(snapshot.task_ref);
            }
        }
        Ok(task_refs)
    }

    pub async fn accept_command(
        &self,
        device_ref: DeviceRef,
        initiated_by: OperatorRef,
        request_id: RequestId,
        command: &CommandTaskSpec,
        execution_context: pab_protocol::ExecutionContext,
        accepted_at_unix_ms: i64,
    ) -> Result<AcceptTaskOutcome, TaskStoreError> {
        let mut tx = self.pool.begin().await?;
        if let Some(row) = sqlx::query(
            "SELECT snapshot_json, command_json FROM task_records WHERE initiated_by = ? AND request_id = ?",
        )
        .bind(initiated_by.storage_key())
        .bind(request_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        {
            let stored_command: CommandTaskSpec = serde_json::from_str(row.get("command_json"))?;
            if &stored_command != command {
                return Err(TaskStoreError::RequestConflict);
            }
            let snapshot = decode_snapshot(row.get("snapshot_json"))?;
            tx.commit().await?;
            return Ok(AcceptTaskOutcome::Existing(snapshot));
        }

        let task_ref = TaskRef {
            device_ref,
            task_id: TaskId::new(),
        };
        let (aggregate, event) = TaskAggregate::accept(AcceptedTask {
            task_ref,
            request_id,
            initiated_by,
            capability: CapabilityRef {
                name: "process.exec".to_owned(),
                version: u32::from(command.options.required_version()),
            },
            display_summary: command.display_summary.clone(),
            execution_context,
            accepted_at_unix_ms,
        })?;
        let snapshot = aggregate.snapshot().clone();
        sqlx::query(
            "INSERT INTO task_records (task_id, initiated_by, request_id, snapshot_json, command_json) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(task_ref.task_id.to_string())
        .bind(initiated_by.storage_key())
        .bind(request_id.to_string())
        .bind(serde_json::to_string(&snapshot)?)
        .bind(serde_json::to_string(command)?)
        .execute(&mut *tx)
        .await?;
        insert_event(&mut tx, &event).await?;
        for stream in [OutputStream::Stdout, OutputStream::Stderr] {
            sqlx::query("INSERT INTO task_outputs (task_id, stream, bytes) VALUES (?, ?, ?)")
                .bind(task_ref.task_id.to_string())
                .bind(stream_name(stream))
                .bind(Vec::<u8>::new())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        self.publish(task_ref, TaskChangeKind::Event);
        Ok(AcceptTaskOutcome::Created(snapshot))
    }

    pub async fn existing_command(
        &self,
        actor: OperatorRef,
        id: RequestId,
        command: &CommandTaskSpec,
    ) -> Result<Option<TaskSnapshot>, TaskStoreError> {
        let row=sqlx::query("SELECT snapshot_json, command_json FROM task_records WHERE initiated_by = ? AND request_id = ?")
            .bind(actor.storage_key()).bind(id.to_string()).fetch_optional(&self.pool).await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let stored: CommandTaskSpec = serde_json::from_str(row.get("command_json"))?;
        if &stored != command {
            return Err(TaskStoreError::RequestConflict);
        }
        decode_snapshot(row.get("snapshot_json")).map(Some)
    }

    pub async fn get_task(
        &self,
        initiated_by: OperatorRef,
        task_ref: TaskRef,
    ) -> Result<TaskSnapshot, TaskStoreError> {
        let row = sqlx::query(
            "SELECT snapshot_json FROM task_records WHERE task_id = ? AND initiated_by = ?",
        )
        .bind(task_ref.task_id.to_string())
        .bind(initiated_by.storage_key())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(TaskStoreError::NotFound)?;
        let snapshot = decode_snapshot(row.get("snapshot_json"))?;
        if snapshot.task_ref != task_ref {
            return Err(TaskStoreError::NotFound);
        }
        Ok(snapshot)
    }

    pub async fn events_after(
        &self,
        initiated_by: OperatorRef,
        task_ref: TaskRef,
        after_seq: u64,
    ) -> Result<Vec<TaskEvent>, TaskStoreError> {
        self.get_task(initiated_by, task_ref).await?;
        let after_seq = i64::try_from(after_seq).map_err(|_| TaskStoreError::OffsetTooLarge)?;
        let rows = sqlx::query(
            "SELECT event_json FROM task_events WHERE task_id = ? AND seq > ? ORDER BY seq ASC",
        )
        .bind(task_ref.task_id.to_string())
        .bind(after_seq)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| serde_json::from_str(row.get("event_json")).map_err(Into::into))
            .collect()
    }

    pub async fn record_event(
        &self,
        task_ref: TaskRef,
        kind: TaskEventKind,
        occurred_at_unix_ms: i64,
    ) -> Result<TaskSnapshot, TaskStoreError> {
        let mut tx = self.pool.begin().await?;
        let mut aggregate = load_aggregate(&mut tx, task_ref).await?;
        let event = aggregate.record(kind, occurred_at_unix_ms)?;
        save_snapshot(&mut tx, aggregate.snapshot()).await?;
        insert_event(&mut tx, &event).await?;
        tx.commit().await?;
        self.publish(task_ref, TaskChangeKind::Event);
        Ok(aggregate.snapshot().clone())
    }

    pub async fn append_output(
        &self,
        task_ref: TaskRef,
        stream: OutputStream,
        bytes: &[u8],
    ) -> Result<OutputRange, TaskStoreError> {
        if bytes.is_empty() {
            return self.output_range(task_ref, stream).await;
        }
        let mut tx = self.pool.begin().await?;
        let mut aggregate = load_aggregate(&mut tx, task_ref).await?;
        let previous = output_range(aggregate.snapshot(), stream);
        let added = u64::try_from(bytes.len()).map_err(|_| TaskStoreError::OffsetTooLarge)?;
        let available_to = previous
            .available_to
            .checked_add(added)
            .ok_or(TaskStoreError::OffsetTooLarge)?;
        let retained_length = previous
            .available_to
            .checked_sub(previous.retained_from)
            .ok_or(TaskStoreError::OffsetTooLarge)?;
        let combined_length = retained_length
            .checked_add(added)
            .ok_or(TaskStoreError::OffsetTooLarge)?;
        let trimmed = combined_length.saturating_sub(MAX_RETAINED_OUTPUT_BYTES);
        let retained_from = previous
            .retained_from
            .checked_add(trimmed)
            .ok_or(TaskStoreError::OffsetTooLarge)?;
        aggregate.observe_output(stream, retained_from, available_to, false)?;
        let sqlite_start =
            i64::try_from(trimmed.saturating_add(1)).map_err(|_| TaskStoreError::OffsetTooLarge)?;
        sqlx::query(
            "UPDATE task_outputs SET bytes = substr(CAST(bytes || ? AS BLOB), ?) WHERE task_id = ? AND stream = ?",
        )
        .bind(bytes)
        .bind(sqlite_start)
        .bind(task_ref.task_id.to_string())
        .bind(stream_name(stream))
        .execute(&mut *tx)
        .await?;
        save_snapshot(&mut tx, aggregate.snapshot()).await?;
        tx.commit().await?;
        self.publish(task_ref, TaskChangeKind::Output(stream));
        Ok(output_range(aggregate.snapshot(), stream))
    }

    pub async fn complete_output(
        &self,
        task_ref: TaskRef,
        stream: OutputStream,
    ) -> Result<OutputRange, TaskStoreError> {
        let mut tx = self.pool.begin().await?;
        let mut aggregate = load_aggregate(&mut tx, task_ref).await?;
        let previous = output_range(aggregate.snapshot(), stream);
        aggregate.observe_output(stream, previous.retained_from, previous.available_to, true)?;
        save_snapshot(&mut tx, aggregate.snapshot()).await?;
        tx.commit().await?;
        self.publish(task_ref, TaskChangeKind::Output(stream));
        Ok(output_range(aggregate.snapshot(), stream))
    }

    pub async fn read_output(
        &self,
        initiated_by: OperatorRef,
        task_ref: TaskRef,
        stream: OutputStream,
        offset: u64,
        max_bytes: u32,
    ) -> Result<(OutputChunk, OutputRange), TaskStoreError> {
        // Keep the range and bytes in one SQLite read snapshot. Output can be
        // appended or trimmed between awaits, including by another process.
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT snapshot_json FROM task_records WHERE task_id = ? AND initiated_by = ?",
        )
        .bind(task_ref.task_id.to_string())
        .bind(initiated_by.storage_key())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(TaskStoreError::NotFound)?;
        let snapshot = decode_snapshot(row.get("snapshot_json"))?;
        if snapshot.task_ref != task_ref {
            return Err(TaskStoreError::NotFound);
        }
        let range = output_range(&snapshot, stream);
        if offset < range.retained_from || offset > range.available_to {
            return Err(TaskStoreError::InvalidOutputOffset);
        }
        let relative = offset
            .checked_sub(range.retained_from)
            .and_then(|value| value.checked_add(1))
            .ok_or(TaskStoreError::OffsetTooLarge)?;
        let relative = i64::try_from(relative).map_err(|_| TaskStoreError::OffsetTooLarge)?;
        let length = i64::from(max_bytes);
        let row = sqlx::query(
            "SELECT substr(bytes, ?, ?) AS chunk FROM task_outputs WHERE task_id = ? AND stream = ?",
        )
        .bind(relative)
        .bind(length)
        .bind(task_ref.task_id.to_string())
        .bind(stream_name(stream))
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((
            OutputChunk {
                schema_version: TASK_SCHEMA_VERSION,
                task_ref,
                stream,
                offset,
                bytes: row.get("chunk"),
            },
            range,
        ))
    }

    async fn output_range(
        &self,
        task_ref: TaskRef,
        stream: OutputStream,
    ) -> Result<OutputRange, TaskStoreError> {
        let row = sqlx::query("SELECT snapshot_json FROM task_records WHERE task_id = ?")
            .bind(task_ref.task_id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or(TaskStoreError::NotFound)?;
        let snapshot = decode_snapshot(row.get("snapshot_json"))?;
        if snapshot.task_ref != task_ref {
            return Err(TaskStoreError::NotFound);
        }
        Ok(output_range(&snapshot, stream))
    }

    fn publish(&self, task_ref: TaskRef, kind: TaskChangeKind) {
        let _ = self.changes.send(TaskChange { task_ref, kind });
    }
}

async fn load_aggregate(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    task_ref: TaskRef,
) -> Result<TaskAggregate, TaskStoreError> {
    let row = sqlx::query("SELECT snapshot_json FROM task_records WHERE task_id = ?")
        .bind(task_ref.task_id.to_string())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(TaskStoreError::NotFound)?;
    let snapshot = decode_snapshot(row.get("snapshot_json"))?;
    if snapshot.task_ref != task_ref {
        return Err(TaskStoreError::NotFound);
    }
    Ok(TaskAggregate::restore(snapshot)?)
}

async fn save_snapshot(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    snapshot: &TaskSnapshot,
) -> Result<(), TaskStoreError> {
    sqlx::query("UPDATE task_records SET snapshot_json = ? WHERE task_id = ?")
        .bind(serde_json::to_string(snapshot)?)
        .bind(snapshot.task_ref.task_id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn insert_event(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    event: &TaskEvent,
) -> Result<(), TaskStoreError> {
    let seq = i64::try_from(event.seq).map_err(|_| TaskStoreError::OffsetTooLarge)?;
    sqlx::query("INSERT INTO task_events (task_id, seq, event_json) VALUES (?, ?, ?)")
        .bind(event.task_ref.task_id.to_string())
        .bind(seq)
        .bind(serde_json::to_string(event)?)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn decode_snapshot(value: &str) -> Result<TaskSnapshot, TaskStoreError> {
    let snapshot: TaskSnapshot = serde_json::from_str(value)?;
    TaskAggregate::restore(snapshot.clone())?;
    Ok(snapshot)
}

fn output_range(snapshot: &TaskSnapshot, stream: OutputStream) -> OutputRange {
    match stream {
        OutputStream::Stdout => snapshot.output.stdout.clone(),
        OutputStream::Stderr => snapshot.output.stderr.clone(),
    }
}

const fn stream_name(stream: OutputStream) -> &'static str {
    match stream {
        OutputStream::Stdout => "stdout",
        OutputStream::Stderr => "stderr",
    }
}

#[derive(Debug, Error)]
pub(crate) enum TaskStoreError {
    #[error("task was not found")]
    NotFound,
    #[error("the request ID was already used with different command arguments")]
    RequestConflict,
    #[error("the requested output offset is outside the retained range")]
    InvalidOutputOffset,
    #[error("task output offset is too large")]
    OffsetTooLarge,
    #[error(transparent)]
    Sql(#[from] sqlx::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Runtime(#[from] TaskRuntimeError),
    #[error("task output read failed: {0}")]
    Io(std::io::Error),
}
