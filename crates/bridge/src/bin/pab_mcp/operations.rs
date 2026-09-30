use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use pab_agent_core::{DataPaths, DataScope};
use pab_bridge::{
    BridgeConfig, BridgeIdentity, BridgeRuntime, QueuedTransfer, TransferControl, TransferQueue,
    TransferRequest,
    desktop_presence::{McpReporter, OperationReport, RuntimeReport},
};
use pab_protocol::{DeviceCode, RequestId};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    sync::{Mutex, watch},
    task::JoinHandle,
};

type RuntimeSlot = Arc<Mutex<Option<Arc<BridgeRuntime>>>>;
type MonitorSlot = Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>;
const MAX_JOBS: usize = 8;

struct Job {
    control: TransferControl,
    cancel: watch::Sender<bool>,
    done: watch::Receiver<bool>,
    task: JoinHandle<()>,
}

pub(super) struct OperationManager {
    pub queue: TransferQueue,
    runtime_slot: RuntimeSlot,
    monitor: MonitorSlot,
    reporter: McpReporter,
    jobs: Mutex<HashMap<RequestId, Job>>,
    rechecks: Mutex<HashMap<RequestId, JoinHandle<()>>>,
    closed: AtomicBool,
    filesystem_rotation: AtomicU64,
    heartbeat: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

pub(super) fn handles(name: &str) -> bool {
    matches!(
        name,
        "pab_upload_file"
            | "pab_download_file"
            | "pab_get_operation"
            | "pab_list_operations"
            | "pab_cancel_operation"
            | "pab_disconnect"
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileArgs {
    device_code: DeviceCode,
    source: String,
    destination: String,
    #[serde(default)]
    overwrite: bool,
    request_id: Option<RequestId>,
    #[serde(default)]
    wait_ms: u64,
}

impl OperationManager {
    pub async fn new(
        runtime_slot: RuntimeSlot,
        monitor: MonitorSlot,
        reporter: McpReporter,
    ) -> Result<Arc<Self>, String> {
        let paths = DataPaths::for_scope(DataScope::User).map_err(|e| e.to_string())?;
        let database = std::env::var_os("PAB_BRIDGE_DATABASE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| paths.bridge_database());
        let actor = if std::env::var("PAB_MCP_GUEST").as_deref() == Ok("0") {
            match BridgeConfig::from_env()
                .map_err(|e| e.to_string())?
                .identity
            {
                BridgeIdentity::Account(id) => format!("account:{id}"),
                BridgeIdentity::Guest => "guest".to_owned(),
            }
        } else {
            "guest".to_owned()
        };
        let queue = TransferQueue::open(&database, RequestId::new().to_string(), actor)
            .await
            .map_err(|e| e.to_string())?;
        let manager = Arc::new(Self {
            queue: queue.clone(),
            runtime_slot,
            monitor,
            reporter,
            jobs: Mutex::new(HashMap::new()),
            rechecks: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            filesystem_rotation: AtomicU64::new(0),
            heartbeat: Mutex::new(None),
        });
        let weak = Arc::downgrade(&manager);
        let task = tokio::spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(5));
            loop {
                timer.tick().await;
                let Some(manager) = weak.upgrade() else {
                    break;
                };
                if manager.closed.load(Ordering::Acquire) {
                    break;
                }
                if let Err(error) = queue.heartbeat().await {
                    tracing::warn!(%error, "MCP operation heartbeat failed");
                }
                manager.report_queue().await;
                if let Ok(mut files) = queue.active_filesystems().await {
                    if !files.is_empty() {
                        let start = manager
                            .filesystem_rotation
                            .fetch_add(MAX_JOBS as u64, Ordering::Relaxed)
                            as usize
                            % files.len();
                        files.rotate_left(start);
                    }
                    for (id, code) in files {
                        manager.recheck(id, code).await;
                    }
                }
                if let Ok(records) = queue.page(None, Some("running"), None, 100).await {
                    for record in records.into_iter().filter(|r| r.phase == "unconfirmed") {
                        if let (Ok(id), Some(code)) =
                            (record.operation.id.parse(), record.operation.device_code)
                        {
                            manager.recheck(id, code).await;
                        }
                    }
                }
                if let Ok(records) = queue.page(None, Some("cancel_requested"), None, 100).await {
                    for record in records {
                        if let (Ok(id), Some(code)) =
                            (record.operation.id.parse(), record.operation.device_code)
                            && !manager.jobs.lock().await.contains_key(&id)
                        {
                            manager.recheck(id, code).await;
                        }
                    }
                }
            }
        });
        *manager.heartbeat.lock().await = Some(task);
        Ok(manager)
    }

    pub async fn runtime(&self) -> Result<Arc<BridgeRuntime>, String> {
        let runtime = {
            let mut slot = self.runtime_slot.lock().await;
            if self.closed.load(Ordering::Acquire) {
                return Err("MCP is shutting down".to_owned());
            }
            super::ensure_runtime(&mut slot, self.queue.session_id()).await?
        };
        let mut monitor = self.monitor.lock().await;
        if monitor.is_none() {
            *monitor = Some(self.reporter.attach_runtime(runtime.presence_source()));
        }
        Ok(runtime)
    }

    /// Report durable operations before networking initializes. Once the
    /// Runtime monitor attaches, it owns reporting and includes the same rows.
    async fn report_queue(&self) {
        let Ok(monitor) = self.monitor.try_lock() else {
            return;
        };
        if monitor.is_some() {
            return;
        }
        let Ok(records) = self.queue.page(None, None, None, 100).await else {
            return;
        };
        let initializing = records
            .iter()
            .any(|r| r.operation.finished_at_unix_ms.is_none());
        let last_error = records.iter().find_map(|r| r.operation.message.clone());
        let operations = records
            .into_iter()
            .map(|record| {
                let op = record.operation;
                OperationReport {
                    id: op.id,
                    device_ref: op.device_ref,
                    device_code: op.device_code.map(|c| c.to_string()),
                    kind: op.kind,
                    direction: op.direction,
                    source: op.source,
                    destination: op.destination,
                    state: if op.state == "running"
                        && op.execution_observation.as_deref() == Some("unconfirmed")
                    {
                        "unconfirmed".to_owned()
                    } else {
                        op.state
                    },
                    completed_bytes: op.offset,
                    total_bytes: op.size,
                    started_at_unix_ms: op.started_at_unix_ms,
                    finished_at_unix_ms: op.finished_at_unix_ms,
                }
            })
            .collect();
        self.reporter.set_runtime(RuntimeReport {
            session_id: self.queue.session_id().to_owned(),
            identity: self.queue.initiated_by().to_owned(),
            control_phase: if initializing {
                "initializing"
            } else {
                "not_initialized"
            }
            .to_owned(),
            operations,
            last_error,
            ..Default::default()
        });
    }

    pub async fn call(self: &Arc<Self>, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "pab_upload_file" | "pab_download_file" => self.submit(name, args).await,
            "pab_get_operation" => self.get(args).await,
            "pab_list_operations" => self.list(args).await,
            "pab_cancel_operation" => self.cancel(args).await,
            "pab_disconnect" => self.disconnect(args).await,
            _ => Err("unknown operation tool".to_owned()),
        }
    }

    async fn submit(self: &Arc<Self>, name: &str, args: &Value) -> Result<Value, String> {
        let args: FileArgs = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
        if args.wait_ms > 5000 {
            return Err("wait_ms must be between 0 and 5000".to_owned());
        }
        let device = self
            .queue
            .remembered_device(args.device_code)
            .await
            .map_err(|_| {
                "Connect this device in Desktop first so its identity and credential are saved"
                    .to_owned()
            })?;
        let upload = name == "pab_upload_file";
        let local = if upload {
            &args.source
        } else {
            &args.destination
        };
        let remote = if upload {
            &args.destination
        } else {
            &args.source
        };
        if !Path::new(local).is_absolute()
            || Path::new(local).file_name().is_none()
            || !remote_absolute(remote, device.os_family)
        {
            return Err("source and destination must be absolute file paths for their respective operating systems".to_owned());
        }
        let spec = TransferRequest {
            request_id: args.request_id.unwrap_or_default(),
            device_ref: device.device_ref,
            device_code: args.device_code,
            direction: if upload { "upload" } else { "download" }.to_owned(),
            source: args.source,
            destination: args.destination,
            overwrite: args.overwrite,
        };
        let mut jobs = self.jobs.lock().await;
        if self.closed.load(Ordering::Acquire) {
            return Err("MCP is shutting down".to_owned());
        }
        // Idempotent requests are allowed even when all worker slots are occupied.
        let existing = self.queue.get(spec.request_id, spec.device_code).await.ok();
        if existing.is_none()
            && self.queue.active_count().await.map_err(|e| e.to_string())? >= MAX_JOBS as i64
        {
            return Err("MCP transfer capacity reached; resolve an existing operation before submitting more".to_owned());
        }
        let (record, inserted) = self.queue.submit(&spec).await.map_err(|e| e.to_string())?;
        self.report_queue().await;
        let mut done = None;
        if inserted {
            let control = TransferControl::default();
            let (cancel, cancelled) = watch::channel(false);
            let (finished, receiver) = watch::channel(false);
            let (start_tx, start_rx) = tokio::sync::oneshot::channel();
            let manager = self.clone();
            let worker_spec = spec.clone();
            let worker_control = control.clone();
            let task = tokio::spawn(async move {
                if start_rx.await.is_ok() {
                    manager.run(worker_spec, worker_control, cancelled).await;
                }
                manager.jobs.lock().await.remove(&spec.request_id);
                let _ = finished.send(true);
            });
            jobs.insert(
                spec.request_id,
                Job {
                    control,
                    cancel,
                    done: receiver.clone(),
                    task,
                },
            );
            done = Some(receiver);
            let _ = start_tx.send(());
        } else if let Some(job) = jobs.get(&spec.request_id) {
            done = Some(job.done.clone());
        }
        drop(jobs);
        if args.wait_ms > 0
            && let Some(mut done) = done
        {
            let _ = tokio::time::timeout(Duration::from_millis(args.wait_ms), async {
                while !*done.borrow_and_update() {
                    if done.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await;
        }
        let record = self
            .queue
            .get(spec.request_id, spec.device_code)
            .await
            .unwrap_or(record);
        Ok(transfer_result(record, &device.os_reminder))
    }

    async fn run(
        self: &Arc<Self>,
        spec: TransferRequest,
        control: TransferControl,
        mut cancelled: watch::Receiver<bool>,
    ) {
        let id = spec.request_id;
        let offset = Arc::new(AtomicU64::new(0));
        let size = Arc::new(AtomicU64::new(0));
        let count = offset.clone();
        let total = size.clone();
        let mut operation = Box::pin(async {
            self.queue
                .phase(id, "connecting", None)
                .await
                .map_err(|e| e.to_string())?;
            let runtime = self.runtime().await?;
            runtime
                .execute_queued_transfer(&spec, &control, move |n, all| {
                    count.store(n, Ordering::Release);
                    total.store(all, Ordering::Release);
                })
                .await
                .map_err(|e| e.to_string())
        });
        let mut timer = tokio::time::interval(Duration::from_millis(250));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last = (0, 0);
        let result = loop {
            tokio::select! {
                biased;
                outcome = &mut operation => break outcome,
                _ = cancelled.changed() => {
                    control.cancel();
                    if control.committing() { break operation.as_mut().await; }
                    break Err("transfer cancellation requested".to_owned());
                },
                _ = timer.tick() => {
                    let progress = (offset.load(Ordering::Acquire), size.load(Ordering::Acquire));
                    if progress != last {
                        if let Err(e) = self.queue.progress(id, progress.0, progress.1).await { tracing::warn!(%e, "cannot persist transfer progress"); }
                        let phase = if progress.0 == progress.1 { "verifying" } else { "transferring" };
                        let _ = self.queue.phase(id, phase, control.digest().as_deref()).await;
                        last = progress;
                    }
                }
            }
        };
        // Drop the stream before trying to reconcile cancellation with the Executor.
        drop(operation);
        let _ = self
            .queue
            .progress(
                id,
                offset.load(Ordering::Acquire),
                size.load(Ordering::Acquire),
            )
            .await;
        let state = match &result {
            Ok(()) => "completed",
            Err(_)
                if control.remote_requested()
                    && (spec.direction == "upload" || control.committing()) =>
            {
                "unconfirmed"
            }
            Err(_) if control.cancelled() => "cancelled",
            Err(_) => "failed",
        };
        let _ = self
            .queue
            .phase(id, state, control.digest().as_deref())
            .await;
        if let Err(error) = &result
            && let Err(e) = self.queue.note(id, error).await
        {
            tracing::warn!(%e,%id,"cannot persist unresolved transfer error");
        }
        let mut needs_recheck = state == "unconfirmed";
        if !needs_recheck
            && let Err(e) = self
                .queue
                .finish(id, state, result.as_ref().err().map(String::as_str))
                .await
        {
            tracing::error!(%e, %id, "cannot persist transfer outcome");
            let _ = self
                .queue
                .phase(id, "unconfirmed", control.digest().as_deref())
                .await;
            needs_recheck = true;
        }
        if needs_recheck {
            self.recheck(id, spec.device_code).await;
        }
        self.report_queue().await;
    }

    async fn recheck(self: &Arc<Self>, id: RequestId, code: DeviceCode) {
        let mut rechecks = self.rechecks.lock().await;
        if self.closed.load(Ordering::Acquire)
            || rechecks.len() >= MAX_JOBS
            || rechecks.contains_key(&id)
        {
            return;
        }
        let manager = self.clone();
        let task = tokio::spawn(async move {
            let lookup = async {
                if let Ok((device, _)) = manager.queue.system_record(id, code).await {
                    manager
                        .runtime()
                        .await?
                        .get_system_query(device, id)
                        .await
                        .map_err(|e| e.to_string())?;
                    return Ok(());
                }
                if let Ok((device, reply)) = manager.queue.filesystem_record(id, code).await {
                    if matches!(
                        reply.state.as_str(),
                        "running" | "unconfirmed" | "cancel_requested"
                    ) {
                        manager
                            .runtime()
                            .await?
                            .get_filesystem(device, id)
                            .await
                            .map_err(|e| e.to_string())?;
                    }
                    return Ok(());
                }
                let record = manager
                    .queue
                    .get(id, code)
                    .await
                    .map_err(|e| e.to_string())?;
                if record.operation.finished_at_unix_ms.is_some() {
                    return Ok(());
                }
                let runtime = manager.runtime().await?;
                let snapshot = runtime
                    .transfer_status(record.operation.device_ref, id)
                    .await
                    .map_err(|e| e.to_string())?;
                manager
                    .queue
                    .reconcile(&record, &snapshot)
                    .await
                    .map_err(|e| e.to_string())
            };
            let _ = tokio::time::timeout(Duration::from_secs(15), lookup).await;
            manager.rechecks.lock().await.remove(&id);
        });
        rechecks.insert(id, task);
    }

    async fn get(self: &Arc<Self>, args: &Value) -> Result<Value, String> {
        let (id, code) = reference(args)?;
        if let Ok((device, cached)) = self.queue.system_record(id, code).await {
            let result = self
                .runtime()
                .await?
                .get_system_query(device, id)
                .await
                .unwrap_or(cached);
            return Ok(
                json!({"operation_ref":{"device_code":code,"operation_id":id,"kind":result.kind},"result":result}),
            );
        }
        if let Ok((device_ref, mut reply)) = self.queue.filesystem_record(id, code).await {
            if matches!(
                reply.state.as_str(),
                "running" | "unconfirmed" | "cancel_requested"
            ) {
                let lookup = async {
                    self.runtime()
                        .await?
                        .get_filesystem(device_ref, id)
                        .await
                        .map_err(|error| error.to_string())
                };
                if let Ok(Ok(current)) = tokio::time::timeout(Duration::from_secs(5), lookup).await
                {
                    reply = current;
                }
            }
            let device = self
                .queue
                .remembered_device(code)
                .await
                .map_err(|error| error.to_string())?;
            return Ok(
                json!({ "operation_ref": { "device_code": code, "operation_id": id, "kind": reply.kind }, "result": reply, "os_reminder": device.os_reminder }),
            );
        }
        if let Ok(record) = self.queue.get(id, code).await {
            if (record.phase == "unconfirmed"
                || record.operation.execution_observation.as_deref() == Some("unconfirmed"))
                && !self.jobs.lock().await.contains_key(&id)
            {
                self.recheck(id, code).await;
            }
            let device = self
                .queue
                .remembered_device(code)
                .await
                .map_err(|e| e.to_string())?;
            return Ok(transfer_result(record, &device.os_reminder));
        }
        let task = self
            .queue
            .command(id, code)
            .await
            .map_err(|e| e.to_string())?;
        let target = task
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.target_context());
        Ok(
            json!({ "operation_ref": { "device_code": code, "operation_id": id, "kind": "command", "task_id": task.snapshot.as_ref().map(|s| s.task_ref.task_id) }, "snapshot": task.snapshot, "complete": task.is_complete(), "stdout": task.stdout, "stderr": task.stderr, "target": target, "os_reminder": target.as_ref().map(|t| t.compact_reminder()), "os_context_source": "task_snapshot" }),
        )
    }

    async fn list(&self, args: &Value) -> Result<Value, String> {
        let code = args
            .get("device_code")
            .and_then(Value::as_str)
            .map(str::parse)
            .transpose()
            .map_err(|_| "invalid device_code".to_owned())?;
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20) as u32;
        let cursor = args
            .get("before")
            .and_then(Value::as_str)
            .map(parse_cursor)
            .transpose()?;
        let mut records = self
            .queue
            .operation_page(
                code,
                args.get("state").and_then(Value::as_str),
                cursor.as_ref().map(|(time, id)| (*time, id.as_str())),
                limit + 1,
            )
            .await
            .map_err(|e| e.to_string())?;
        let mut more = records.len() > limit as usize;
        records.truncate(limit as usize);
        // Leave room for the envelope. Cursors follow the last returned entry,
        // including when the byte budget rather than count closes the page.
        let mut bytes = 0;
        let mut within_budget = 0;
        for record in &records {
            bytes += serde_json::to_vec(record).map_err(|e| e.to_string())?.len();
            if bytes > 240 * 1024 {
                break;
            }
            within_budget += 1;
        }
        if within_budget < records.len() {
            more = true;
            records.truncate(within_budget);
        }
        let next = if more {
            records.last().map(|r| {
                format!(
                    "{}:{}",
                    r["started_at_unix_ms"].as_i64().unwrap(),
                    r["operation_id"].as_str().unwrap()
                )
            })
        } else {
            None
        };
        Ok(
            json!({ "operations": records, "next_before": next, "scope": "current_mcp_session", "truncated": more }),
        )
    }

    async fn cancel(self: &Arc<Self>, args: &Value) -> Result<Value, String> {
        let (id, code) = reference(args)?;
        if self.queue.system_record(id, code).await.is_ok() {
            return Err("this system operation cannot be cancelled or rolled back; query the original operation_ref for its result".to_owned());
        }
        if let Ok((device, reply)) = self.queue.filesystem_record(id, code).await {
            if !pab_protocol::cancellable_filesystem_kind(&reply.kind) {
                return Err("this bounded filesystem operation does not support cancellation; query the original operation_ref".to_owned());
            }
            if !self
                .queue
                .owns_filesystem(id)
                .await
                .map_err(|e| e.to_string())?
            {
                return Err("cannot cancel another MCP session's operation".to_owned());
            }
            let runtime = self.runtime().await?;
            let reply = tokio::time::timeout(Duration::from_secs(5), runtime.cancel_filesystem(device, id)).await.map_err(|_| "cancellation outcome is unconfirmed; query or retry cancellation using the original operation_ref")?.map_err(|e| e.to_string())?;
            return Ok(
                json!({ "operation_ref": { "device_code": code, "operation_id": id, "kind": reply.kind }, "result": reply }),
            );
        }
        if let Ok(record) = self.queue.get(id, code).await {
            if !record.owned {
                return Err("cannot cancel another MCP session's operation".to_owned());
            }
            let record = self
                .queue
                .cancel(id, code)
                .await
                .map_err(|e| e.to_string())?;
            let needs_recheck = {
                let jobs = self.jobs.lock().await;
                if let Some(job) = jobs.get(&id) {
                    job.control.cancel();
                    let _ = job.cancel.send(true);
                    false // The worker will reconcile only if it contacted the Executor.
                } else {
                    record.operation.finished_at_unix_ms.is_none()
                }
            };
            if needs_recheck {
                self.recheck(id, code).await;
            }
            return Ok(transfer_result(
                record,
                &self
                    .queue
                    .remembered_device(code)
                    .await
                    .map_err(|e| e.to_string())?
                    .os_reminder,
            ));
        }
        if !self.queue.owns_task(id).await.map_err(|e| e.to_string())? {
            return Err("cannot cancel another MCP session's task".to_owned());
        }
        let task = self
            .queue
            .command(id, code)
            .await
            .map_err(|e| e.to_string())?;
        let task_ref = task
            .snapshot
            .ok_or("task has not yet been accepted")?
            .task_ref;
        let runtime = self.runtime().await?;
        let snapshot = tokio::time::timeout(
            Duration::from_secs(2),
            runtime.cancel_task(task_ref, "MCP cancellation requested".to_owned()),
        )
        .await
        .map_err(|_| {
            "task cancellation outcome is unconfirmed; query the original operation".to_owned()
        })?
        .map_err(|e| e.to_string())?;
        Ok(
            json!({ "operation_ref": { "device_code": code, "operation_id": id, "kind": "command" }, "snapshot": snapshot }),
        )
    }

    async fn disconnect(&self, args: &Value) -> Result<Value, String> {
        let code: DeviceCode = args
            .get("device_code")
            .and_then(Value::as_str)
            .ok_or("missing device_code")?
            .parse()
            .map_err(|_| "invalid device_code")?;
        let _jobs = self.jobs.lock().await;
        if self
            .queue
            .active_for_device(code)
            .await
            .map_err(|e| e.to_string())?
            != 0
        {
            return Err("device has unresolved operations; cancel or finish them and confirm their outcomes before disconnecting".to_owned());
        }
        let device = self
            .queue
            .remembered_device(code)
            .await
            .map_err(|e| e.to_string())?;
        let runtime = self
            .runtime_slot
            .try_lock()
            .map_err(|_| "MCP runtime is initializing; retry disconnect")?
            .clone();
        if let Some(runtime) = runtime {
            runtime.disconnect_device(device.device_ref).await;
        }
        Ok(
            json!({ "device_code": code, "disconnected": true, "scope": "current_mcp_session", "os_reminder": device.os_reminder }),
        )
    }

    pub async fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        if let Some(task) = self.heartbeat.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
        let jobs = self.jobs.lock().await.drain().collect::<Vec<_>>();
        for (_, job) in &jobs {
            job.control.cancel();
            let _ = job.cancel.send(true);
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        for (id, mut job) in jobs {
            if tokio::time::timeout_at(deadline, &mut job.task)
                .await
                .is_err()
            {
                job.task.abort();
                let _ = job.task.await;
            }
            let _ = self
                .queue
                .phase(id, "unconfirmed", job.control.digest().as_deref())
                .await;
        }
        let rechecks = self
            .rechecks
            .lock()
            .await
            .drain()
            .map(|(_, task)| task)
            .collect::<Vec<_>>();
        for task in &rechecks {
            task.abort();
        }
        for task in rechecks {
            let _ = task.await;
        }
        let runtime = self
            .runtime_slot
            .try_lock()
            .ok()
            .and_then(|slot| slot.clone());
        if let Some(runtime) = runtime
            && let Ok(files) = self.queue.active_filesystems().await
        {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            for (id, code) in files {
                if let Ok((device, reply)) = self.queue.filesystem_record(id, code).await
                    && pab_protocol::cancellable_filesystem_kind(&reply.kind)
                {
                    let _ =
                        tokio::time::timeout_at(deadline, runtime.cancel_filesystem(device, id))
                            .await;
                }
            }
        }
        let _ = self.queue.close().await;
    }
}

fn transfer_result(mut record: QueuedTransfer, reminder: &str) -> Value {
    if record.phase == "unconfirmed" && record.operation.finished_at_unix_ms.is_none() {
        record.operation.execution_observation = Some("unconfirmed".to_owned());
    }
    if record.operation.finished_at_unix_ms.is_some() {
        record.phase = record.operation.state.clone();
    }
    json!({ "operation_ref": { "device_code": record.operation.device_code, "operation_id": record.operation.id, "kind": "file_transfer", "device_ref": record.operation.device_ref }, "complete": record.operation.finished_at_unix_ms.is_some(), "operation": record, "os_reminder": reminder, "os_context_source": "remembered_device" })
}

fn reference(args: &Value) -> Result<(RequestId, DeviceCode), String> {
    let id = args
        .get("operation_id")
        .and_then(Value::as_str)
        .ok_or("missing operation_id")?
        .parse()
        .map_err(|_| "invalid operation_id")?;
    let code = args
        .get("device_code")
        .and_then(Value::as_str)
        .ok_or("missing device_code")?
        .parse()
        .map_err(|_| "invalid device_code")?;
    Ok((id, code))
}

fn parse_cursor(value: &str) -> Result<(i64, String), String> {
    let (time, id) = value.split_once(':').ok_or("invalid before cursor")?;
    let time = time.parse().map_err(|_| "invalid before timestamp")?;
    let id: RequestId = id.parse().map_err(|_| "invalid before operation ID")?;
    Ok((time, id.to_string()))
}

fn remote_absolute(path: &str, os: pab_protocol::OsFamily) -> bool {
    let bytes = path.as_bytes();
    if path.is_empty() || path.contains('\0') {
        return false;
    }
    match os {
        pab_protocol::OsFamily::Windows => {
            (bytes.len() > 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'\\' | b'/'))
                || (path.starts_with("\\\\")
                    && path[2..]
                        .split(['\\', '/'])
                        .filter(|part| !part.is_empty())
                        .count()
                        >= 3)
        }
        _ => path.starts_with('/') && path.len() > 1,
    }
}
