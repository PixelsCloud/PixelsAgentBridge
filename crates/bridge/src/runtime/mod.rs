mod credential;
mod device;
mod event;
mod remembered;
mod store;
#[cfg(test)]
mod store_tests;
mod worker;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pab_protocol::{
    CommandTaskSpec, DeviceCode, DeviceRef, ExpectedEnvironment, OutputChunk, OutputRange,
    OutputStream, RequestId, TargetContext, TaskEvent, TaskRef, TaskSnapshot,
};
use thiserror::Error;
use tokio::{
    sync::{Mutex, broadcast, oneshot, watch},
    task::{AbortHandle, JoinHandle},
};

use crate::{BridgeConfig, BridgeConfigError, BridgeError};
use pab_agent_core::{DeviceNetworkResolutionError, EndpointControlError};

use device::{BridgeAvailability, DeviceSession, run_bridge_supervisor};
use worker::run_record;

pub use credential::{
    DevicePasswordProvider, DirectoryDevicePasswordProvider, FileDevicePasswordProvider,
    MemoryDevicePasswordProvider, RuntimeCredentialError,
};
pub use event::{DeviceConnectionPhase, RuntimeEvent, RuntimeEventKind};
pub use remembered::RememberedDevice;
use store::RuntimeStore;
pub use store::{LocalTaskRecord, RuntimeStoreError};

pub struct BridgeLocalStore {
    store: RuntimeStore,
}

impl BridgeLocalStore {
    pub async fn open(path: &std::path::Path) -> Result<Self, RuntimeStoreError> {
        Ok(Self {
            store: RuntimeStore::open(path).await?,
        })
    }

    pub async fn remembered_devices(&self) -> Result<Vec<RememberedDevice>, RuntimeStoreError> {
        self.store.remembered_devices().await
    }

    pub async fn tasks(&self) -> Result<Vec<LocalTaskRecord>, RuntimeStoreError> {
        self.store.list().await
    }

    pub async fn task(&self, task_ref: TaskRef) -> Result<LocalTaskRecord, RuntimeStoreError> {
        self.store.get_by_task(task_ref).await
    }

    pub async fn read_output(
        &self,
        task_ref: TaskRef,
        stream: OutputStream,
        offset: u64,
        max_bytes: u32,
    ) -> Result<(OutputChunk, OutputRange), RuntimeStoreError> {
        self.store
            .read_output(task_ref, stream, offset, max_bytes)
            .await
    }
}

const DEFAULT_RETRY_INTERVAL: Duration = Duration::from_secs(3);
const DEFAULT_EVENT_BUFFER: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeRuntimeConfig {
    pub database_path: PathBuf,
    pub retry_interval: Duration,
    pub event_buffer: usize,
    pub resume_incomplete_on_start: bool,
}

impl BridgeRuntimeConfig {
    pub fn new(database_path: impl Into<PathBuf>) -> Self {
        Self {
            database_path: database_path.into(),
            retry_interval: DEFAULT_RETRY_INTERVAL,
            event_buffer: DEFAULT_EVENT_BUFFER,
            resume_incomplete_on_start: true,
        }
    }

    fn validate(&self) -> Result<(), RuntimeError> {
        if self.retry_interval.is_zero() {
            return Err(RuntimeError::InvalidConfig(
                "retry_interval must be greater than zero".to_owned(),
            ));
        }
        if self.event_buffer == 0 {
            return Err(RuntimeError::InvalidConfig(
                "event_buffer must be greater than zero".to_owned(),
            ));
        }
        Ok(())
    }
}

pub struct BridgeRuntime {
    inner: Arc<RuntimeInner>,
    shutdown: watch::Sender<bool>,
    bridge_task: JoinHandle<()>,
}

impl BridgeRuntime {
    pub async fn list_devices(
        &self,
    ) -> Result<Vec<pab_protocol::DeviceDirectoryEntry>, RuntimeError> {
        let mut availability = self.inner.availability.clone();
        loop {
            let state = availability.borrow().clone();
            match state {
                BridgeAvailability::Connected(connector) => {
                    return connector.list_devices().await.map_err(Into::into);
                }
                BridgeAvailability::Stopped(message) => {
                    return Err(RuntimeError::BridgeUnavailable(message));
                }
                BridgeAvailability::Connecting => {}
            }
            availability.changed().await.map_err(|_| {
                RuntimeError::BridgeUnavailable("Bridge supervisor stopped".to_owned())
            })?;
        }
    }

    pub async fn start(
        bridge_config: BridgeConfig,
        runtime_config: BridgeRuntimeConfig,
        passwords: Arc<dyn DevicePasswordProvider>,
    ) -> Result<Self, RuntimeError> {
        bridge_config.validate()?;
        runtime_config.validate()?;
        let store = RuntimeStore::open(&runtime_config.database_path).await?;
        let (events, _) = broadcast::channel(runtime_config.event_buffer);
        let (availability_sender, availability) = watch::channel(BridgeAvailability::Connecting);
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let guest_identity = matches!(bridge_config.identity, crate::BridgeIdentity::Guest);
        let inner = Arc::new(RuntimeInner {
            store,
            passwords,
            retry_interval: runtime_config.retry_interval,
            events,
            event_sequence: AtomicU64::new(0),
            availability,
            shutdown: shutdown_receiver,
            devices: Mutex::new(HashMap::new()),
            guest_codes: Mutex::new(HashSet::new()),
            guest_identity,
            operations: Mutex::new(HashMap::new()),
        });
        inner.publish(RuntimeEventKind::BridgeConnecting);
        let bridge_task = tokio::spawn(run_bridge_supervisor(
            bridge_config,
            runtime_config.retry_interval,
            availability_sender,
            shutdown.subscribe(),
            Arc::clone(&inner),
        ));
        let runtime = Self {
            inner,
            shutdown,
            bridge_task,
        };
        if runtime_config.resume_incomplete_on_start {
            for record in runtime.inner.store.incomplete().await? {
                runtime.start_record(record, None).await?;
            }
        }
        Ok(runtime)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.inner.events.subscribe()
    }

    pub async fn resolve_device_code(&self, code: DeviceCode) -> Result<DeviceRef, RuntimeError> {
        let mut availability = self.inner.availability.clone();
        loop {
            let state = availability.borrow().clone();
            match state {
                BridgeAvailability::Connected(connector) => {
                    match connector.resolve_device_code(code).await {
                        Ok(device_ref) => {
                            if self.inner.guest_identity {
                                self.inner.start_guest_lease(code).await;
                            }
                            return Ok(device_ref);
                        }
                        Err(BridgeError::NetworkResolution(
                            DeviceNetworkResolutionError::Unavailable
                            | DeviceNetworkResolutionError::Timeout,
                        )) => {
                            self.inner
                                .wait_or_shutdown(self.inner.retry_interval)
                                .await?;
                            continue;
                        }
                        Err(BridgeError::NetworkResolution(
                            DeviceNetworkResolutionError::Control(EndpointControlError::Server {
                                code: pab_protocol::ControlErrorCode::Internal,
                                ..
                            }),
                        )) => {
                            self.inner
                                .wait_or_shutdown(self.inner.retry_interval)
                                .await?;
                            continue;
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                BridgeAvailability::Stopped(message) => {
                    return Err(RuntimeError::BridgeUnavailable(message));
                }
                BridgeAvailability::Connecting => {}
            }
            availability.changed().await.map_err(|_| {
                RuntimeError::BridgeUnavailable("Bridge supervisor stopped".to_owned())
            })?;
        }
    }

    pub async fn submit_command(
        &self,
        device_ref: DeviceRef,
        request_id: RequestId,
        program: String,
        args: Vec<String>,
        cwd: Option<String>,
    ) -> Result<TaskSnapshot, RuntimeError> {
        let target = self.current_environment(device_ref).await?;
        let display_summary = display_summary(&program, &args);
        let command = CommandTaskSpec {
            program,
            args,
            cwd,
            expected_environment: ExpectedEnvironment {
                os_family: target.execution.os_family,
                environment_revision: target.execution.environment_revision.clone(),
            },
            display_summary,
        };
        self.submit_command_spec(device_ref, request_id, command)
            .await
    }

    pub async fn submit_command_spec(
        &self,
        device_ref: DeviceRef,
        request_id: RequestId,
        command: CommandTaskSpec,
    ) -> Result<TaskSnapshot, RuntimeError> {
        let record = self
            .inner
            .store
            .record_pending(device_ref, request_id, &command)
            .await?;
        if let Some(snapshot) = record.snapshot.clone() {
            if !record.is_complete() {
                let _ = self.start_record(record, None).await;
            }
            return Ok(snapshot);
        }
        let mut events = self.subscribe();
        let (sender, receiver) = oneshot::channel();
        match self.start_record(record, Some(sender)).await {
            Ok(()) => receiver
                .await
                .map_err(|_| RuntimeError::WorkerStopped)?
                .map_err(RuntimeError::TaskOperation),
            Err(RuntimeError::OperationAlreadyRunning(_)) => {
                self.wait_for_acceptance(request_id, &mut events).await
            }
            Err(error) => Err(error),
        }
    }

    pub async fn current_environment(
        &self,
        device_ref: DeviceRef,
    ) -> Result<TargetContext, RuntimeError> {
        let device = self.inner.device(device_ref).await;
        loop {
            let connection = device.connection().await?;
            match connection.get_environment().await {
                Ok(target) => return Ok(target),
                Err(error) if error.is_recoverable_connection() => {
                    device.recover(&connection, &error).await?;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub async fn upload_file(
        &self,
        device_ref: DeviceRef,
        source: &std::path::Path,
        destination: &str,
        overwrite: bool,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), RuntimeError> {
        let connection = self.inner.device(device_ref).await.connection().await?;
        connection
            .upload_file(source, destination, overwrite, progress)
            .await?;
        Ok(())
    }

    pub async fn download_file(
        &self,
        device_ref: DeviceRef,
        source: &str,
        destination: &std::path::Path,
        overwrite: bool,
        progress: impl Fn(u64, u64) + Send + Sync,
    ) -> Result<(), RuntimeError> {
        let connection = self.inner.device(device_ref).await.connection().await?;
        connection
            .download_file(source, destination, overwrite, progress)
            .await?;
        Ok(())
    }

    pub async fn cancel_task(
        &self,
        task_ref: TaskRef,
        reason: String,
    ) -> Result<TaskSnapshot, RuntimeError> {
        let record = self.inner.store.get_by_task(task_ref).await?;
        let device = self.inner.device(task_ref.device_ref).await;
        loop {
            let connection = device.connection().await?;
            match connection.cancel_task(task_ref, reason.clone()).await {
                Ok(snapshot) => {
                    let updated = self.inner.store.update_snapshot(&snapshot).await?;
                    self.inner.publish_snapshot(&snapshot);
                    if !updated.is_complete() {
                        let _ = self.start_record(updated, None).await;
                    }
                    return Ok(snapshot);
                }
                Err(error) if error.is_recoverable_connection() => {
                    self.inner.publish_task_retry(
                        record.snapshot.as_ref().map(TaskSnapshot::target_context),
                        record.request_id,
                        &error,
                    );
                    device.recover(&connection, &error).await?;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub async fn follow_task(&self, task_ref: TaskRef) -> Result<TaskSnapshot, RuntimeError> {
        match self.inner.store.get_by_task(task_ref).await {
            Ok(record) => {
                let snapshot = record
                    .snapshot
                    .clone()
                    .ok_or(RuntimeError::MissingTaskSnapshot)?;
                if !record.is_complete() {
                    let _ = self.start_record(record, None).await;
                }
                return Ok(snapshot);
            }
            Err(error) if error.is_not_found() => {}
            Err(error) => return Err(error.into()),
        }
        let device = self.inner.device(task_ref.device_ref).await;
        let snapshot = loop {
            let connection = device.connection().await?;
            match connection.get_task(task_ref).await {
                Ok(snapshot) => break snapshot,
                Err(error) if error.is_recoverable_connection() => {
                    device.recover(&connection, &error).await?;
                }
                Err(error) => return Err(error.into()),
            }
        };
        let record = self.inner.store.adopt_snapshot(&snapshot).await?;
        self.inner.publish_snapshot(&snapshot);
        if !record.is_complete() {
            self.start_record(record, None).await?;
        }
        Ok(snapshot)
    }

    pub async fn task_by_request(
        &self,
        request_id: RequestId,
    ) -> Result<LocalTaskRecord, RuntimeError> {
        Ok(self.inner.store.get_by_request(request_id).await?)
    }

    pub async fn task(&self, task_ref: TaskRef) -> Result<LocalTaskRecord, RuntimeError> {
        Ok(self.inner.store.get_by_task(task_ref).await?)
    }

    pub async fn tasks(&self) -> Result<Vec<LocalTaskRecord>, RuntimeError> {
        Ok(self.inner.store.list().await?)
    }

    pub async fn remembered_devices(&self) -> Result<Vec<RememberedDevice>, RuntimeError> {
        Ok(self.inner.store.remembered_devices().await?)
    }

    pub async fn remember_device(&self, device: &RememberedDevice) -> Result<(), RuntimeError> {
        Ok(self.inner.store.remember_device(device).await?)
    }

    pub async fn rename_device(&self, code: DeviceCode, alias: &str) -> Result<(), RuntimeError> {
        Ok(self.inner.store.rename_device(code, alias).await?)
    }

    pub async fn resume_incomplete_for_device(
        &self,
        device_ref: DeviceRef,
    ) -> Result<(), RuntimeError> {
        for record in self.inner.store.incomplete().await? {
            if record.device_ref == device_ref {
                match self.start_record(record, None).await {
                    Ok(()) | Err(RuntimeError::OperationAlreadyRunning(_)) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
    }

    pub async fn events_after(
        &self,
        task_ref: TaskRef,
        after_seq: u64,
    ) -> Result<Vec<TaskEvent>, RuntimeError> {
        Ok(self.inner.store.events_after(task_ref, after_seq).await?)
    }

    pub async fn read_output(
        &self,
        task_ref: TaskRef,
        stream: OutputStream,
        offset: u64,
        max_bytes: u32,
    ) -> Result<(OutputChunk, OutputRange), RuntimeError> {
        Ok(self
            .inner
            .store
            .read_output(task_ref, stream, offset, max_bytes)
            .await?)
    }

    pub async fn shutdown(self) -> Result<(), RuntimeError> {
        let _ = self.shutdown.send(true);
        let operations = self
            .inner
            .operations
            .lock()
            .await
            .drain()
            .map(|(_, handle)| handle)
            .collect::<Vec<_>>();
        for operation in operations {
            operation.abort();
        }
        self.bridge_task.await?;
        Ok(())
    }

    async fn start_record(
        &self,
        record: LocalTaskRecord,
        accepted: Option<oneshot::Sender<Result<TaskSnapshot, String>>>,
    ) -> Result<(), RuntimeError> {
        let request_id = record.request_id;
        let mut operations = self.inner.operations.lock().await;
        if operations.contains_key(&request_id) {
            return Err(RuntimeError::OperationAlreadyRunning(request_id));
        }
        let inner = Arc::clone(&self.inner);
        let task = tokio::spawn(async move {
            let mut accepted = accepted;
            let result = run_record(Arc::clone(&inner), record, &mut accepted).await;
            if let Err(error) = result {
                let message = error.to_string();
                if let Some(sender) = accepted.take() {
                    let _ = sender.send(Err(message.clone()));
                }
                let target = inner
                    .store
                    .get_by_request(request_id)
                    .await
                    .ok()
                    .and_then(|record| record.snapshot.map(|snapshot| snapshot.target_context()));
                inner.publish(RuntimeEventKind::TaskStopped {
                    target,
                    request_id,
                    message,
                });
            }
            inner.operations.lock().await.remove(&request_id);
        });
        operations.insert(request_id, task.abort_handle());
        Ok(())
    }

    async fn wait_for_acceptance(
        &self,
        request_id: RequestId,
        events: &mut broadcast::Receiver<RuntimeEvent>,
    ) -> Result<TaskSnapshot, RuntimeError> {
        loop {
            let record = self.inner.store.get_by_request(request_id).await?;
            if let Some(snapshot) = record.snapshot {
                return Ok(snapshot);
            }
            if !self.inner.operations.lock().await.contains_key(&request_id) {
                return Err(RuntimeError::WorkerStopped);
            }
            match events.recv().await {
                Ok(RuntimeEvent {
                    kind:
                        RuntimeEventKind::TaskStopped {
                            request_id: stopped,
                            message,
                            ..
                        },
                    ..
                }) if stopped == request_id => {
                    return Err(RuntimeError::TaskOperation(message));
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(RuntimeError::WorkerStopped);
                }
            }
        }
    }
}

struct RuntimeInner {
    store: RuntimeStore,
    passwords: Arc<dyn DevicePasswordProvider>,
    retry_interval: Duration,
    events: broadcast::Sender<RuntimeEvent>,
    event_sequence: AtomicU64,
    availability: watch::Receiver<BridgeAvailability>,
    shutdown: watch::Receiver<bool>,
    devices: Mutex<HashMap<DeviceRef, Arc<DeviceSession>>>,
    guest_codes: Mutex<HashSet<DeviceCode>>,
    guest_identity: bool,
    operations: Mutex<HashMap<RequestId, AbortHandle>>,
}

impl RuntimeInner {
    async fn start_guest_lease(self: &Arc<Self>, code: DeviceCode) {
        if !self.guest_codes.lock().await.insert(code) {
            return;
        }
        let inner = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut delay = Duration::from_secs(180);
            loop {
                let Some(runtime) = inner.upgrade() else {
                    return;
                };
                if runtime.wait_or_shutdown(delay).await.is_err() {
                    return;
                }
                let state = runtime.availability.borrow().clone();
                let refreshed = match state {
                    BridgeAvailability::Connected(connector) => {
                        connector.resolve_device_code(code).await.is_ok()
                    }
                    _ => false,
                };
                delay = if refreshed {
                    Duration::from_secs(180)
                } else {
                    runtime.retry_interval
                };
            }
        });
    }

    async fn device(self: &Arc<Self>, device_ref: DeviceRef) -> Arc<DeviceSession> {
        let mut devices = self.devices.lock().await;
        Arc::clone(
            devices
                .entry(device_ref)
                .or_insert_with(|| Arc::new(DeviceSession::new(device_ref, Arc::clone(self)))),
        )
    }

    fn publish(&self, kind: RuntimeEventKind) {
        let sequence = self.event_sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.events.send(RuntimeEvent {
            sequence,
            occurred_at_unix_ms: unix_millis(),
            kind,
        });
    }

    fn publish_snapshot(&self, snapshot: &TaskSnapshot) {
        self.publish(RuntimeEventKind::TaskSnapshot {
            target: snapshot.target_context(),
            snapshot: Box::new(snapshot.clone()),
        });
    }

    fn publish_task_retry(
        &self,
        target: Option<TargetContext>,
        request_id: RequestId,
        error: &BridgeError,
    ) {
        if let Some(target) = target {
            self.publish(RuntimeEventKind::TaskRetrying {
                target,
                request_id,
                retry_in_ms: duration_millis(self.retry_interval),
                message: error.to_string(),
            });
        }
    }

    async fn wait_or_shutdown(&self, duration: Duration) -> Result<(), RuntimeError> {
        let mut shutdown = self.shutdown.clone();
        tokio::select! {
            _ = tokio::time::sleep(duration) => Ok(()),
            changed = shutdown.changed() => {
                let _ = changed;
                Err(RuntimeError::ShuttingDown)
            }
        }
    }
}

fn display_summary(program: &str, args: &[String]) -> String {
    let mut summary = program.to_owned();
    if !args.is_empty() {
        summary.push_str(" …");
    }
    summary.chars().take(512).collect()
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

pub(super) fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Config(#[from] BridgeConfigError),
    #[error(transparent)]
    Store(#[from] RuntimeStoreError),
    #[error(transparent)]
    Credential(#[from] RuntimeCredentialError),
    #[error(transparent)]
    Bridge(#[from] BridgeError),
    #[error("Bridge Runtime configuration is invalid: {0}")]
    InvalidConfig(String),
    #[error("Bridge connection is unavailable: {0}")]
    BridgeUnavailable(String),
    #[error("Bridge Runtime is shutting down")]
    ShuttingDown,
    #[error("request {0} is already being processed")]
    OperationAlreadyRunning(RequestId),
    #[error("the task worker stopped before accepting the command")]
    WorkerStopped,
    #[error("the target execution environment changed before command submission")]
    EnvironmentChanged,
    #[error("an accepted local task record has no remote snapshot")]
    MissingTaskSnapshot,
    #[error("a pending local task record has no command specification")]
    MissingPendingCommand,
    #[error("remote task operation failed: {0}")]
    TaskOperation(String),
    #[error("Executor returned an unexpected task response: {0}")]
    UnexpectedResponse(String),
    #[error("Bridge Runtime task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}
