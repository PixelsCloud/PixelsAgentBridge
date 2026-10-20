//! Durable command facts, shared task state machine and bounded transient output.
//! Shells have the Android application UID, not adb/root privileges or a file sandbox.
use crate::{peer::now_ms, storage::StorageBridge};
use pab_protocol::*;
use pab_task_runtime::{AcceptedTask, TaskAggregate};
use pab_transport::{PabBiStream, PabConnectionError};
use serde_json::json;
use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, Notify, Semaphore, watch};

mod terminal;
pub(crate) use terminal::ConnectionScope;
#[cfg(test)]
mod tests;

const OUTPUT_LIMIT: usize = 1024 * 1024;
type Result<T> = std::result::Result<T, ExecError>;
#[derive(Debug)]
pub(super) struct ExecError(pub DeviceTaskErrorCode, pub &'static str);
impl From<std::io::Error> for ExecError {
    fn from(_: std::io::Error) -> Self {
        Self(DeviceTaskErrorCode::Internal, "command_io_failed")
    }
}
impl From<sqlx::Error> for ExecError {
    fn from(_: sqlx::Error) -> Self {
        Self(
            DeviceTaskErrorCode::StorageUnavailable,
            "execution_journal_unavailable",
        )
    }
}
impl ExecError {
    fn wire(self) -> DeviceTaskResponse {
        DeviceTaskResponse::Error {
            code: self.0,
            message: self.1.into(),
        }
    }
}

#[derive(Default)]
struct Output {
    start: u64,
    bytes: VecDeque<u8>,
}
impl Output {
    fn append(&mut self, bytes: &[u8]) {
        if bytes.len() >= OUTPUT_LIMIT {
            self.start += (self.bytes.len() + bytes.len() - OUTPUT_LIMIT) as u64;
            self.bytes.clear();
            self.bytes.extend(&bytes[bytes.len() - OUTPUT_LIMIT..]);
        } else {
            let count = (self.bytes.len() + bytes.len()).saturating_sub(OUTPUT_LIMIT);
            self.bytes.drain(..count);
            self.start += count as u64;
            self.bytes.extend(bytes);
        }
    }
    fn range(&self, complete: bool) -> OutputRange {
        OutputRange {
            retained_from: self.start,
            available_to: self.start + self.bytes.len() as u64,
            complete,
        }
    }
}
struct LiveTask {
    aggregate: TaskAggregate,
    stdout: Output,
    stderr: Output,
    cancel: watch::Sender<Option<String>>,
}

pub(crate) struct ExecutionService {
    pub sdk: Arc<StorageBridge>,
    pool: SqlitePool,
    key: [u8; 32],
    device: DeviceRef,
    acceptance: Mutex<()>,
    live: Mutex<HashMap<TaskId, Arc<Mutex<LiveTask>>>>,
    slots: Arc<Semaphore>,
    requests: Arc<Semaphore>,
    changed: Notify,
    stopping: std::sync::atomic::AtomicBool,
    terminals: Mutex<HashMap<RequestId, Arc<terminal::Entry>>>,
}

impl ExecutionService {
    pub async fn open(path: &Path, key: [u8; 32], device: DeviceRef) -> Result<Arc<Self>> {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(2));
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await?;
        for sql in [
            "CREATE TABLE IF NOT EXISTS command_tasks(owner TEXT NOT NULL,id TEXT NOT NULL,task TEXT UNIQUE NOT NULL,digest TEXT NOT NULL,snapshot TEXT NOT NULL,PRIMARY KEY(owner,id))",
            "CREATE TABLE IF NOT EXISTS command_events(task TEXT NOT NULL,seq INTEGER NOT NULL,event TEXT NOT NULL,PRIMARY KEY(task,seq))",
            "CREATE TABLE IF NOT EXISTS terminals(owner TEXT NOT NULL,id TEXT PRIMARY KEY,digest TEXT NOT NULL,state TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS terminal_actions(id TEXT NOT NULL,seq INTEGER NOT NULL,digest TEXT NOT NULL,state TEXT NOT NULL,PRIMARY KEY(id,seq))",
        ] {
            sqlx::query(sql).execute(&pool).await?;
        }
        for row in sqlx::query("SELECT task,snapshot FROM command_tasks")
            .fetch_all(&pool)
            .await?
        {
            let snapshot: TaskSnapshot = serde_json::from_str(row.get("snapshot"))
                .map_err(|_| internal("execution_journal_invalid"))?;
            if !snapshot.state.is_terminal() {
                let mut aggregate = TaskAggregate::restore(snapshot)
                    .map_err(|_| internal("execution_journal_invalid"))?;
                let event = aggregate
                    .record(
                        TaskEventKind::Interrupted {
                            reason: "process_restarted_outcome_unconfirmed_original_not_replayed"
                                .into(),
                        },
                        now_ms(),
                    )
                    .map_err(|_| internal("execution_journal_invalid"))?;
                let mut saved = aggregate.snapshot().clone();
                expire_outputs(&mut saved);
                let mut tx = pool.begin().await?;
                sqlx::query("UPDATE command_tasks SET snapshot=? WHERE task=?")
                    .bind(json!(saved).to_string())
                    .bind(row.get::<String, _>("task"))
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("INSERT INTO command_events(task,seq,event) VALUES(?,?,?)")
                    .bind(saved.task_ref.task_id.to_string())
                    .bind(event.seq as i64)
                    .bind(json!(event).to_string())
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
            }
        }
        sqlx::query(
            "UPDATE terminals SET state='interrupted' WHERE state IN ('accepted','running')",
        )
        .execute(&pool)
        .await?;
        let service = Arc::new(Self {
            sdk: StorageBridge::for_domain("execution"),
            pool,
            key,
            device,
            acceptance: Mutex::new(()),
            live: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(4)),
            requests: Arc::new(Semaphore::new(16)),
            changed: Notify::new(),
            stopping: std::sync::atomic::AtomicBool::new(false),
            terminals: Mutex::new(HashMap::new()),
        });
        let weak = Arc::downgrade(&service);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(service) = weak.upgrade() else { break };
                if service.stopping.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                if service.has_active().await && service.authorize().await.is_err() {
                    service.cancel_all("control_unavailable_or_paused").await;
                    service.close_terminals(None).await;
                }
                service.expire_terminals().await;
            }
        });
        Ok(service)
    }

    pub(crate) async fn authorize(&self) -> Result<(PathBuf, u32)> {
        if self.stopping.load(std::sync::atomic::Ordering::Acquire) {
            return Err(denied("execution_stopping"));
        }
        let value = tokio::time::timeout(
            Duration::from_secs(2),
            self.sdk.call(json!({"action":"authorize"})),
        )
        .await
        .map_err(|_| denied("execution_authorization_unavailable"))?
        .map_err(|_| denied("execution_authorization_unavailable"))?;
        if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
            return Err(denied(match error {
                "control_paused" => "control_paused",
                _ => "control_disconnected",
            }));
        }
        let root = PathBuf::from(
            value["workspace"]
                .as_str()
                .ok_or_else(|| internal("execution_workspace_unavailable"))?,
        );
        let uid = value["uid"]
            .as_u64()
            .filter(|uid| *uid <= u32::MAX as u64)
            .ok_or_else(|| internal("execution_uid_unavailable"))? as u32;
        if !root.is_absolute()
            || !root.is_dir()
            || root.symlink_metadata()?.file_type().is_symlink()
        {
            return Err(denied("invalid_execution_workspace"));
        }
        Ok((root, uid))
    }

    async fn has_active(&self) -> bool {
        if !self.terminals.lock().await.is_empty() {
            return true;
        }
        let entries: Vec<_> = self.live.lock().await.values().cloned().collect();
        for task in entries {
            if !task.lock().await.aggregate.snapshot().state.is_terminal() {
                return true;
            }
        }
        false
    }

    pub async fn shutdown(&self) {
        self.stopping
            .store(true, std::sync::atomic::Ordering::Release);
        let _accept = self.acceptance.lock().await;
        self.cancel_all("connection_service_stopped").await;
        self.close_terminals(None).await;
        let _ = tokio::time::timeout(Duration::from_secs(8), async {
            while self.has_active().await {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await;
    }

    /// Once a request reaches the service, transport cancellation cannot drop it
    /// between durable acceptance and dispatch. Detached work remains bounded.
    async fn detached<T: Send + 'static>(
        &self,
        work: impl std::future::Future<Output = Result<T>> + Send + 'static,
    ) -> Result<T> {
        let permit = self
            .requests
            .clone()
            .try_acquire_owned()
            .map_err(|_| denied("execution_request_busy"))?;
        tokio::spawn(async move {
            let _permit = permit;
            work.await
        })
        .await
        .map_err(|_| internal("execution_request_outcome_unconfirmed"))?
    }

    async fn cancel_all(&self, reason: &str) {
        let entries: Vec<_> = self.live.lock().await.values().cloned().collect();
        for task in entries {
            let mut task = task.lock().await;
            if !task.aggregate.snapshot().state.is_terminal() {
                if task.aggregate.snapshot().state != TaskState::CancelRequested {
                    let _ = self.record(&mut task, TaskEventKind::CancelRequested).await;
                }
                task.cancel.send_replace(Some(reason.into()));
            }
        }
    }

    fn digest(&self, bytes: &[u8]) -> String {
        blake3::keyed_hash(&self.key, bytes).to_hex().to_string()
    }

    async fn submit(
        self: &Arc<Self>,
        owner: &str,
        peer: &AuthorizedDevicePeer,
        context: &ExecutionContext,
        id: RequestId,
        original: CommandTaskSpec,
    ) -> Result<TaskSnapshot> {
        validate(&original)?;
        let digest =
            self.digest(&serde_json::to_vec(&original).map_err(|_| invalid("invalid_command"))?);
        let _accept = self.acceptance.lock().await;
        if let Some(row) =
            sqlx::query("SELECT digest,task FROM command_tasks WHERE owner=? AND id=?")
                .bind(owner)
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await?
        {
            if row.get::<String, _>("digest") != digest {
                return Err(ExecError(
                    DeviceTaskErrorCode::RequestConflict,
                    "request_conflict",
                ));
            }
            let task_id: TaskId = row
                .get::<String, _>("task")
                .parse()
                .map_err(|_| internal("execution_journal_invalid"))?;
            return self
                .snapshot(
                    owner,
                    TaskRef {
                        device_ref: self.device,
                        task_id,
                    },
                )
                .await;
        }
        if !context.matches_expected(&original.expected_environment) {
            return Err(ExecError(
                DeviceTaskErrorCode::EnvironmentChanged,
                "target_execution_environment_changed",
            ));
        }
        if !original.options.execution.is_service() {
            return Err(ExecError(
                DeviceTaskErrorCode::Unsupported,
                "android_commands_require_service_execution",
            ));
        }
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| denied("command_busy"))?;
        let (root, uid) = self.authorize().await?;
        if self.stopping.load(std::sync::atomic::Ordering::Acquire) {
            return Err(denied("execution_stopping"));
        }
        let cwd = resolve_cwd(&root, original.cwd.as_deref().unwrap_or("/workspace"))?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM command_tasks")
            .fetch_one(&self.pool)
            .await?;
        if count >= 10000 {
            return Err(internal("command_journal_capacity"));
        }
        let mut context = context.clone();
        context.cwd = Some(original.cwd.clone().unwrap_or_else(|| "/workspace".into()));
        context.identity = Some(identity(uid, &root));
        let task_ref = TaskRef {
            device_ref: self.device,
            task_id: TaskId::new(),
        };
        let (mut aggregate, event) = TaskAggregate::accept(AcceptedTask {
            task_ref,
            request_id: id,
            initiated_by: peer.operator,
            capability: CapabilityRef {
                name: "process.exec".into(),
                version: u32::from(original.options.required_version()),
            },
            display_summary: original.display_summary.clone(),
            execution_context: context,
            accepted_at_unix_ms: now_ms(),
        })
        .map_err(|_| invalid("invalid_command"))?;
        let mut attributed = aggregate.snapshot().clone();
        attributed.initiating_user = peer.user_context.user.clone();
        aggregate = TaskAggregate::restore(attributed).map_err(|_| invalid("invalid_command"))?;
        let snapshot = aggregate.snapshot().clone();
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO command_tasks(owner,id,task,digest,snapshot) VALUES(?,?,?,?,?)")
            .bind(owner)
            .bind(id.to_string())
            .bind(task_ref.task_id.to_string())
            .bind(digest)
            .bind(json!(snapshot).to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO command_events(task,seq,event) VALUES(?,?,?)")
            .bind(task_ref.task_id.to_string())
            .bind(event.seq as i64)
            .bind(json!(event).to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.evict_completed().await;
        let (cancel, receiver) = watch::channel(None);
        let task = Arc::new(Mutex::new(LiveTask {
            aggregate,
            stdout: Output::default(),
            stderr: Output::default(),
            cancel,
        }));
        self.live
            .lock()
            .await
            .insert(task_ref.task_id, task.clone());
        let mut command = original;
        command.cwd = Some(cwd.to_string_lossy().into_owned());
        let env = environment(&root);
        for (key, value) in env {
            command.options.env.entry(key).or_insert(value);
        }
        command.options.timeout_ms =
            Some(command.options.timeout_ms.unwrap_or(300000).min(86400000));
        let service = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let sink = TaskSink {
                service: service.clone(),
                task: task.clone(),
            };
            if pab_command_runtime::execute(sink.clone(), command, receiver)
                .await
                .is_err()
            {
                let _ = pab_command_runtime::CommandSink::finish_outputs(&sink).await;
                let mut task = task.lock().await;
                if !task.aggregate.snapshot().state.is_terminal() {
                    let _=service.record(&mut task,TaskEventKind::Interrupted{reason:"command_result_or_persistence_unconfirmed_original_not_replayed".into()}).await;
                }
            }
            service.changed.notify_waiters();
        });
        Ok(snapshot)
    }

    async fn evict_completed(&self) {
        let mut live = self.live.lock().await;
        if live.len() < 32 {
            return;
        }
        let mut remove = Vec::new();
        for (id, entry) in live.iter() {
            if entry.lock().await.aggregate.snapshot().state.is_terminal() {
                remove.push(*id);
            }
        }
        for id in remove {
            live.remove(&id);
            if live.len() < 24 {
                break;
            }
        }
    }

    async fn record(&self, task: &mut LiveTask, event: TaskEventKind) -> Result<()> {
        let mut aggregate = task.aggregate.clone();
        let event = aggregate
            .record(event, now_ms())
            .map_err(|_| internal("invalid_command_transition"))?;
        let snapshot = aggregate.snapshot();
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE command_tasks SET snapshot=? WHERE task=?")
            .bind(json!(snapshot).to_string())
            .bind(snapshot.task_ref.task_id.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO command_events(task,seq,event) VALUES(?,?,?)")
            .bind(snapshot.task_ref.task_id.to_string())
            .bind(event.seq as i64)
            .bind(json!(event).to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        task.aggregate = aggregate;
        self.changed.notify_waiters();
        Ok(())
    }

    async fn checked_snapshot(&self, owner: &str, task: TaskRef) -> Result<TaskSnapshot> {
        if task.device_ref != self.device {
            return Err(missing());
        }
        let row = sqlx::query("SELECT snapshot FROM command_tasks WHERE owner=? AND task=?")
            .bind(owner)
            .bind(task.task_id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(missing)?;
        serde_json::from_str(row.get("snapshot")).map_err(|_| internal("execution_journal_invalid"))
    }

    async fn snapshot(&self, owner: &str, task: TaskRef) -> Result<TaskSnapshot> {
        let mut saved = self.checked_snapshot(owner, task).await?;
        if let Some(task) = self.live.lock().await.get(&task.task_id).cloned() {
            return Ok(task.lock().await.aggregate.snapshot().clone());
        }
        expire_outputs(&mut saved);
        Ok(saved)
    }

    async fn output(
        &self,
        owner: &str,
        task: TaskRef,
        which: OutputStream,
        offset: u64,
        max: u32,
    ) -> Result<DeviceTaskResponse> {
        let snapshot = self.checked_snapshot(owner, task).await?;
        let live = self.live.lock().await.get(&task.task_id).cloned();
        let (range, bytes, offset) = if let Some(live) = live {
            let live = live.lock().await;
            let data = match which {
                OutputStream::Stdout => &live.stdout,
                OutputStream::Stderr => &live.stderr,
            };
            let complete = match which {
                OutputStream::Stdout => live.aggregate.snapshot().output.stdout.complete,
                OutputStream::Stderr => live.aggregate.snapshot().output.stderr.complete,
            };
            let range = data.range(complete);
            if offset > range.available_to {
                return Err(invalid("output_offset_beyond_end"));
            }
            let offset = offset.max(range.retained_from);
            let bytes = data
                .bytes
                .iter()
                .skip((offset - range.retained_from) as usize)
                .take(max.min(MAX_OUTPUT_READ_BYTES) as usize)
                .copied()
                .collect();
            (range, bytes, offset)
        } else {
            let mut range = match which {
                OutputStream::Stdout => snapshot.output.stdout,
                OutputStream::Stderr => snapshot.output.stderr,
            };
            range.retained_from = range.available_to;
            range.complete = true;
            if offset > range.available_to {
                return Err(invalid("output_offset_beyond_end"));
            }
            let offset = range.available_to;
            (range, Vec::new(), offset)
        };
        Ok(DeviceTaskResponse::Output {
            chunk: OutputChunk {
                schema_version: TASK_SCHEMA_VERSION,
                task_ref: task,
                stream: which,
                offset,
                bytes,
            },
            range,
        })
    }

    async fn cancel(&self, owner: &str, task: TaskRef, reason: &str) -> Result<TaskSnapshot> {
        let saved = self.checked_snapshot(owner, task).await?;
        if saved.state.is_terminal() {
            return self.snapshot(owner, task).await;
        }
        let live = self
            .live
            .lock()
            .await
            .get(&task.task_id)
            .cloned()
            .ok_or_else(missing)?;
        let mut live = live.lock().await;
        if !live.aggregate.snapshot().state.is_terminal() {
            if live.aggregate.snapshot().state != TaskState::CancelRequested {
                self.record(&mut live, TaskEventKind::CancelRequested)
                    .await?;
            }
            live.cancel
                .send_replace(Some(reason.chars().take(256).collect()));
        }
        Ok(live.aggregate.snapshot().clone())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn serve_request(
        self: &Arc<Self>,
        owner: &str,
        scope: &Arc<ConnectionScope>,
        peer: &AuthorizedDevicePeer,
        context: &ExecutionContext,
        request: &DeviceTaskRequest,
        stream: &mut PabBiStream,
    ) -> std::result::Result<bool, PabConnectionError> {
        if self.serve_terminal(owner, scope, request, stream).await? {
            return Ok(true);
        }
        if let DeviceTaskRequest::Subscribe {
            task_ref,
            after_event_seq,
            stdout_offset,
            stderr_offset,
            ..
        } = request
        {
            self.subscribe(
                owner,
                *task_ref,
                (*after_event_seq, *stdout_offset, *stderr_offset),
                stream,
            )
            .await?;
            return Ok(true);
        }
        let response = match request {
            DeviceTaskRequest::SubmitCommand {
                request_id,
                command,
                ..
            } => {
                let service = self.clone();
                let owner = owner.to_owned();
                let peer = peer.clone();
                let context = context.clone();
                let id = *request_id;
                let command = command.clone();
                self.detached(
                    async move { service.submit(&owner, &peer, &context, id, command).await },
                )
                .await
                .map(|snapshot| DeviceTaskResponse::Submitted {
                    snapshot: Box::new(snapshot),
                })
            }
            DeviceTaskRequest::GetTask { task_ref, .. } => self
                .snapshot(owner, *task_ref)
                .await
                .map(|snapshot| DeviceTaskResponse::Snapshot {
                    snapshot: Box::new(snapshot),
                }),
            DeviceTaskRequest::ReadOutput {
                task_ref,
                stream,
                offset,
                max_bytes,
                ..
            } if *max_bytes > 0 => match self.authorize().await {
                Ok(_) => {
                    self.output(owner, *task_ref, *stream, *offset, *max_bytes)
                        .await
                }
                Err(error) => Err(error),
            },
            DeviceTaskRequest::Cancel {
                task_ref, reason, ..
            } => self.cancel(owner, *task_ref, reason).await.map(|snapshot| {
                DeviceTaskResponse::CancelAccepted {
                    snapshot: Box::new(snapshot),
                }
            }),
            _ => return Ok(false),
        }
        .unwrap_or_else(ExecError::wire);
        stream.send_json(&response, Duration::from_secs(8)).await?;
        Ok(true)
    }

    async fn subscribe(
        &self,
        owner: &str,
        task: TaskRef,
        cursors: (u64, u64, u64),
        stream: &mut PabBiStream,
    ) -> std::result::Result<(), PabConnectionError> {
        let (mut seq, mut stdout, mut stderr) = cursors;
        let timeout = Duration::from_secs(8);
        loop {
            let changed = self.changed.notified();
            let snapshot = match self.snapshot(owner, task).await {
                Ok(value) => value,
                Err(error) => return stream.send_json(&error.wire(), timeout).await,
            };
            let rows = match sqlx::query(
                "SELECT event FROM command_events WHERE task=? AND seq>? ORDER BY seq LIMIT 32",
            )
            .bind(task.task_id.to_string())
            .bind(seq.min(i64::MAX as u64) as i64)
            .fetch_all(&self.pool)
            .await
            {
                Ok(value) => value,
                Err(_) => {
                    return stream
                        .send_json(&internal("execution_journal_unavailable").wire(), timeout)
                        .await;
                }
            };
            for row in rows {
                let event: TaskEvent = serde_json::from_str(row.get("event"))
                    .map_err(|_| PabConnectionError::Stream("invalid task event".into()))?;
                seq = event.seq;
                stream
                    .send_frame_json(&DeviceTaskResponse::Event { event }, timeout)
                    .await?;
            }
            if let Err(error) = self.authorize().await {
                return stream.send_frame_json(&error.wire(), timeout).await;
            }
            for (which, offset) in [
                (OutputStream::Stdout, &mut stdout),
                (OutputStream::Stderr, &mut stderr),
            ] {
                for _ in 0..64 {
                    let response = match self
                        .output(owner, task, which, *offset, MAX_OUTPUT_READ_BYTES)
                        .await
                    {
                        Ok(value) => value,
                        Err(error) => return stream.send_frame_json(&error.wire(), timeout).await,
                    };
                    let DeviceTaskResponse::Output { chunk, range } = &response else {
                        unreachable!()
                    };
                    *offset = chunk.offset + chunk.bytes.len() as u64;
                    if chunk.bytes.is_empty() {
                        stream
                            .send_frame_json(
                                &DeviceTaskResponse::OutputChanged {
                                    task_ref: task,
                                    stream: which,
                                    range: range.clone(),
                                },
                                timeout,
                            )
                            .await?;
                        break;
                    }
                    stream.send_frame_json(&response, timeout).await?;
                }
            }
            let latest = match self.snapshot(owner, task).await {
                Ok(value) => value,
                Err(error) => return stream.send_frame_json(&error.wire(), timeout).await,
            };
            if seq < latest.latest_event_seq
                || stdout < latest.output.stdout.available_to
                || stderr < latest.output.stderr.available_to
            {
                continue;
            }
            stream
                .send_frame_json(
                    &DeviceTaskResponse::CaughtUp {
                        snapshot: Box::new(latest.clone()),
                    },
                    timeout,
                )
                .await?;
            if latest.state.is_terminal() {
                return stream.finish_send(timeout).await;
            }
            let _ = tokio::time::timeout(Duration::from_secs(1), changed).await;
            // Polling fallback closes the Notify registration race and sends a heartbeat.
            let _ = snapshot;
        }
    }
}

#[derive(Clone)]
struct TaskSink {
    service: Arc<ExecutionService>,
    task: Arc<Mutex<LiveTask>>,
}
impl pab_command_runtime::CommandSink for TaskSink {
    type Error = ExecError;
    async fn event(&self, event: TaskEventKind) -> Result<()> {
        let mut task = self.task.lock().await;
        self.service.record(&mut task, event).await
    }
    async fn output(&self, which: OutputStream, bytes: &[u8]) -> Result<()> {
        let mut task = self.task.lock().await;
        let complete = match which {
            OutputStream::Stdout => task.aggregate.snapshot().output.stdout.complete,
            OutputStream::Stderr => task.aggregate.snapshot().output.stderr.complete,
        };
        if complete {
            return Err(internal("command_output_already_completed"));
        }
        let data = match which {
            OutputStream::Stdout => &mut task.stdout,
            OutputStream::Stderr => &mut task.stderr,
        };
        data.append(bytes);
        let range = data.range(false);
        task.aggregate
            .observe_output(which, range.retained_from, range.available_to, false)
            .map_err(|_| internal("invalid_output_transition"))?;
        let snapshot = task.aggregate.snapshot();
        let result = sqlx::query("UPDATE command_tasks SET snapshot=? WHERE task=?")
            .bind(json!(snapshot).to_string())
            .bind(snapshot.task_ref.task_id.to_string())
            .execute(&self.service.pool)
            .await;
        if result.is_err() {
            task.cancel
                .send_replace(Some("output_persistence_unconfirmed".into()));
            return Err(internal("output_persistence_unconfirmed"));
        }
        self.service.changed.notify_waiters();
        Ok(())
    }
    async fn finish_outputs(&self) -> Result<()> {
        let mut task = self.task.lock().await;
        for which in [OutputStream::Stdout, OutputStream::Stderr] {
            let data = match which {
                OutputStream::Stdout => &task.stdout,
                OutputStream::Stderr => &task.stderr,
            };
            let range = data.range(true);
            task.aggregate
                .observe_output(which, range.retained_from, range.available_to, true)
                .map_err(|_| internal("invalid_output_transition"))?;
        }
        Ok(())
    }
}

fn expire_outputs(snapshot: &mut TaskSnapshot) {
    for range in [&mut snapshot.output.stdout, &mut snapshot.output.stderr] {
        range.retained_from = range.available_to;
        range.complete = true;
    }
}
fn invalid(message: &'static str) -> ExecError {
    ExecError(DeviceTaskErrorCode::InvalidRequest, message)
}
fn internal(message: &'static str) -> ExecError {
    ExecError(DeviceTaskErrorCode::Internal, message)
}
fn denied(message: &'static str) -> ExecError {
    ExecError(DeviceTaskErrorCode::AccessDenied, message)
}
fn missing() -> ExecError {
    ExecError(DeviceTaskErrorCode::NotFound, "task_not_found_for_caller")
}

fn validate(command: &CommandTaskSpec) -> Result<()> {
    command
        .options
        .validate()
        .map_err(|_| invalid("invalid_command_options"))?;
    if command.program.trim().is_empty()
        || command.program.len() > MAX_COMMAND_PROGRAM_BYTES
        || command.program.contains('\0')
        || command.args.len() > MAX_COMMAND_ARGUMENTS
        || command
            .args
            .iter()
            .any(|arg| arg.len() > MAX_COMMAND_ARGUMENT_BYTES || arg.contains('\0'))
        || command.display_summary.trim().is_empty()
        || command.display_summary.len() > 512
        || command
            .cwd
            .as_ref()
            .is_some_and(|cwd| cwd.len() > 4096 || cwd.contains('\0'))
        || serde_json::to_vec(command)
            .map_err(|_| invalid("invalid_command"))?
            .len()
            > 48 * 1024
    {
        return Err(invalid("invalid_command"));
    }
    Ok(())
}
fn resolve_cwd(root: &Path, path: &str) -> Result<PathBuf> {
    let relative = path
        .strip_prefix("/workspace")
        .filter(|tail| tail.is_empty() || tail.starts_with('/'))
        .ok_or_else(|| invalid("command_cwd_requires_workspace_use_file_tools_for_saf"))?;
    let relative = relative.trim_start_matches('/');
    if Path::new(relative)
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid("invalid_command_cwd"));
    }
    let mut target = root.to_path_buf();
    for part in Path::new(relative).components() {
        target.push(part);
        if target.symlink_metadata()?.file_type().is_symlink() {
            return Err(denied("command_cwd_symlink_denied"));
        }
    }
    if !target.is_dir() {
        return Err(invalid("command_cwd_not_directory"));
    }
    Ok(target)
}
fn environment(root: &Path) -> BTreeMap<String, String> {
    [
        ("PATH", "/system/bin:/system/xbin"),
        ("HOME", root.to_str().unwrap_or("/")),
        ("TMPDIR", root.to_str().unwrap_or("/")),
        ("SHELL", "/system/bin/sh"),
        ("TERM", "xterm-256color"),
        ("LANG", "C.UTF-8"),
        ("ANDROID_ROOT", "/system"),
        ("ANDROID_DATA", "/data"),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect()
}
fn identity(uid: u32, root: &Path) -> ExecutionIdentity {
    ExecutionIdentity {
        mode: ExecutionMode::Service,
        account_id: format!("uid:{uid}"),
        account_name: format!("Android app uid {uid}"),
        home: root.to_string_lossy().into_owned(),
        primary_group: Some(uid),
        session_id: None,
        logon_id: None,
        environment_source: ExecutionEnvironmentSource::ServiceProcess,
    }
}
