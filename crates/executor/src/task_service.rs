use std::{
    collections::HashMap,
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime},
};

use pab_protocol::{
    CommandTaskSpec, ContextFreshness, DEVICE_TASK_SCHEMA_VERSION, DesktopInputEvent, DeviceRef,
    DeviceTaskErrorCode, DeviceTaskRequest, DeviceTaskResponse, ExecutionContext,
    MAX_COMMAND_ARGUMENT_BYTES, MAX_COMMAND_ARGUMENTS, MAX_COMMAND_PROGRAM_BYTES,
    MAX_OUTPUT_READ_BYTES, OperatorRef, OutputStream, ScreenshotMeta, TargetContext,
    TargetContextSource, TaskError, TaskEventKind, TaskId, TaskRef, TaskSnapshot, WindowList,
};
use pab_task_runtime::TaskRuntimeError;
use pab_transport::{MAX_BINARY_FRAME_BYTES, PabBiStream, PabConnectionError};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::{Mutex, watch};

mod command;
mod directory;
mod file_transfer;
mod filesystem;
mod filesystem_archive;
mod filesystem_bulk;
mod filesystem_bulk_io;
mod filesystem_hash;
mod filesystem_io;
mod filesystem_mkdir;
mod filesystem_publish;
mod filesystem_search;
mod filesystem_text;
mod subscription;
mod system_query;
mod terminal;
mod upload_lock;

use crate::session::ActiveSessions;
use crate::task_store::{AcceptTaskOutcome, TaskStore, TaskStoreError};

const MAX_DISPLAY_SUMMARY_BYTES: usize = 1024;
const MAX_CWD_BYTES: usize = 32 * 1024;
const MAX_CANCEL_REASON_BYTES: usize = 1024;
const MAX_ACTIVE_TASKS: usize = 32;
const MAX_ACTIVE_TERMINALS: usize = 8;

#[derive(Clone)]
pub(crate) struct TaskService {
    store: TaskStore,
    device_ref: DeviceRef,
    execution_context: ExecutionContext,
    active: Arc<Mutex<HashMap<TaskId, watch::Sender<Option<String>>>>>,
    terminals: Arc<Mutex<HashMap<pab_protocol::RequestId, Arc<terminal::ActiveTerminal>>>>,
    upload_locks: upload_lock::UploadPathLocks,
    filesystem_slots: Arc<tokio::sync::Semaphore>,
    #[cfg(test)]
    hash_test_gate: Arc<Mutex<Option<Arc<filesystem_hash::HashTestGate>>>>,
    #[cfg(test)]
    bulk_test_gate: Arc<Mutex<Option<Arc<filesystem_bulk::BulkTestGate>>>>,
    bulk_slots: Arc<tokio::sync::Semaphore>,
    bulk_jobs: Arc<Mutex<HashMap<pab_protocol::RequestId, filesystem_bulk::BulkJob>>>,
    hash_slots: Arc<tokio::sync::Semaphore>,
    hash_jobs: Arc<Mutex<HashMap<pab_protocol::RequestId, filesystem_hash::HashJob>>>,
    system_collector: Arc<std::sync::Mutex<pab_platform::SystemCollector>>,
    system_slots: Arc<tokio::sync::Semaphore>,
    system_jobs: Arc<Mutex<std::collections::HashSet<pab_protocol::RequestId>>>,
    active_sessions: ActiveSessions,
}

impl TaskService {
    pub async fn open(
        database_file: &Path,
        device_ref: DeviceRef,
        execution_context: ExecutionContext,
    ) -> Result<Self, TaskServiceError> {
        let store = TaskStore::open(database_file).await?;
        store.interrupt_transfers().await?;
        store.interrupt_read_operations().await?;
        store.interrupt_terminals().await?;
        for task_ref in store.incomplete_task_refs().await? {
            store
                .complete_output(task_ref, OutputStream::Stdout)
                .await?;
            store
                .complete_output(task_ref, OutputStream::Stderr)
                .await?;
            store
                .record_event(
                    task_ref,
                    TaskEventKind::Interrupted {
                        reason: "Executor restarted before the task result was recorded".to_owned(),
                    },
                    unix_millis()?,
                )
                .await?;
        }
        Ok(Self {
            store,
            device_ref,
            execution_context,
            active: Arc::new(Mutex::new(HashMap::new())),
            terminals: Arc::new(Mutex::new(HashMap::new())),
            upload_locks: upload_lock::UploadPathLocks::default(),
            filesystem_slots: Arc::new(tokio::sync::Semaphore::new(8)),
            #[cfg(test)]
            hash_test_gate: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            bulk_test_gate: Arc::new(Mutex::new(None)),
            bulk_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            bulk_jobs: Arc::new(Mutex::new(HashMap::new())),
            hash_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            hash_jobs: Arc::new(Mutex::new(HashMap::new())),
            system_collector: Arc::new(std::sync::Mutex::new(
                pab_platform::SystemCollector::default(),
            )),
            system_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            system_jobs: Arc::new(Mutex::new(std::collections::HashSet::new())),
            active_sessions: ActiveSessions::default(),
        })
    }

    pub(crate) fn with_active_sessions(mut self, active_sessions: ActiveSessions) -> Self {
        self.active_sessions = active_sessions;
        self
    }

    pub async fn handle_stream(
        &self,
        initiated_by: OperatorRef,
        mut stream: PabBiStream,
        timeout: Duration,
    ) -> Result<(), TaskServiceError> {
        let request: DeviceTaskRequest = stream.receive_json(timeout).await?;
        if request.schema_version() != DEVICE_TASK_SCHEMA_VERSION {
            return send_error(
                &mut stream,
                timeout,
                DeviceTaskErrorCode::InvalidRequest,
                "unsupported device task schema version",
            )
            .await;
        }
        if let DeviceTaskRequest::FileSystem { request, .. } = request {
            return self
                .filesystem_stream(initiated_by, request, &mut stream, timeout)
                .await;
        }
        if let DeviceTaskRequest::Subscribe {
            task_ref,
            after_event_seq,
            stdout_offset,
            stderr_offset,
            ..
        } = request
        {
            return self
                .subscribe_stream(
                    initiated_by,
                    task_ref,
                    after_event_seq,
                    stdout_offset,
                    stderr_offset,
                    stream,
                    timeout,
                )
                .await;
        }

        if matches!(
            &request,
            DeviceTaskRequest::CaptureScreenshot { .. }
                | DeviceTaskRequest::CaptureScreenshotV2 { .. }
        ) {
            let (request_id, options) = match request {
                DeviceTaskRequest::CaptureScreenshot { request_id, .. } => (request_id, None),
                DeviceTaskRequest::CaptureScreenshotV2 {
                    request_id,
                    options,
                    ..
                } => {
                    if let Err(message) = options.validate() {
                        return send_error(
                            &mut stream,
                            timeout,
                            DeviceTaskErrorCode::InvalidRequest,
                            message,
                        )
                        .await;
                    }
                    stream.expect_receive_end(timeout).await?;
                    (request_id, Some(options))
                }
                _ => unreachable!(),
            };
            self.store
                .start_screenshot_read(request_id, initiated_by)
                .await?;
            let result = self
                .capture_screenshot_stream(request_id, options, &mut stream, timeout)
                .await;
            let (state, size, message) = match &result {
                Ok(size) => ("completed", *size, None),
                Err(error) => ("failed", 0, Some(error.to_string())),
            };
            self.store
                .finish_directory_read(request_id, state, size, message.as_deref())
                .await?;
            return match result {
                Ok(_) => Ok(()),
                Err(error) => {
                    let response = error_response(&error);
                    let _ = stream.send_json(&response, timeout).await;
                    Err(error)
                }
            };
        }

        if matches!(
            request,
            DeviceTaskRequest::OpenTerminal { .. }
                | DeviceTaskRequest::TerminalInput { .. }
                | DeviceTaskRequest::TerminalRead { .. }
                | DeviceTaskRequest::TerminalResize { .. }
                | DeviceTaskRequest::TerminalClose { .. }
        ) {
            let result = self
                .handle_terminal(initiated_by, request, &mut stream, timeout)
                .await;
            if let Err(error) = &result {
                let response = error_response(error);
                let _ = stream.send_json(&response, timeout).await;
            }
            return result;
        }

        match &request {
            DeviceTaskRequest::UploadFile {
                request_id,
                path,
                size,
                sha256,
                overwrite,
                ..
            } => {
                self.store
                    .start_transfer(
                        *request_id,
                        initiated_by,
                        "receive",
                        path,
                        *size,
                        Some(sha256),
                    )
                    .await?;
                let result = file_transfer::upload(
                    &self.store,
                    *request_id,
                    &mut stream,
                    timeout.max(Duration::from_secs(60)),
                    path,
                    *size,
                    sha256,
                    *overwrite,
                    &self.upload_locks,
                )
                .await;
                self.finish_transfer(*request_id, &result).await?;
                return result;
            }
            DeviceTaskRequest::DownloadFile {
                request_id,
                path,
                offset,
                ..
            } => {
                self.store
                    .start_transfer(*request_id, initiated_by, "send", path, 0, None)
                    .await?;
                let result = file_transfer::download(
                    &self.store,
                    *request_id,
                    &mut stream,
                    timeout.max(Duration::from_secs(60)),
                    path,
                    *offset,
                )
                .await;
                self.finish_transfer(*request_id, &result).await?;
                return result;
            }
            _ => {}
        }

        let response = match self.handle_request(initiated_by, request).await {
            Ok(response) => response,
            Err(error) => error_response(&error),
        };
        stream.send_json(&response, timeout).await?;
        Ok(())
    }

    async fn capture_screenshot_stream(
        &self,
        request_id: pab_protocol::RequestId,
        options: Option<pab_protocol::ScreenshotOptions>,
        stream: &mut PabBiStream,
        timeout: Duration,
    ) -> Result<usize, TaskServiceError> {
        if !cfg!(any(
            target_os = "windows",
            target_os = "linux",
            target_os = "macos"
        )) {
            return Err(TaskServiceError::Unsupported);
        }
        let (bytes, width, height, format, capture) = if let Some(options) = options {
            let image = crate::local_ipc::request_screenshot_v2(options.clone())
                .await
                .map_err(TaskServiceError::WindowHelper)?;
            let image = tokio::task::spawn_blocking(move || {
                pab_screenshot::verify(&image.bytes, &image.info, &options)?;
                Ok::<_, String>(image)
            })
            .await
            .map_err(|_| TaskServiceError::InvalidRequest("screenshot validation worker stopped"))?
            .map_err(|e| {
                TaskServiceError::WindowHelper(crate::local_ipc::LocalIpcError::Remote(e))
            })?;
            let info = image.info;
            (
                image.bytes,
                info.width,
                info.height,
                info.format.name().to_string(),
                Some(info),
            )
        } else {
            let bytes = crate::local_ipc::request_screenshot()
                .await
                .map_err(TaskServiceError::WindowHelper)?;
            let (w, h) =
                crate::local_ipc::validate_png(&bytes).map_err(TaskServiceError::WindowHelper)?;
            (bytes, w, h, "png".into(), None)
        };
        let meta = ScreenshotMeta {
            request_id,
            format,
            width,
            height,
            size: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(&bytes)),
            capture,
        };
        stream
            .send_frame_json(&DeviceTaskResponse::Screenshot { meta }, timeout)
            .await?;
        for chunk in bytes.chunks(MAX_BINARY_FRAME_BYTES.min(64 * 1024)) {
            stream.send_binary_frame(chunk, timeout).await?;
        }
        stream.finish_send(timeout).await?;
        Ok(bytes.len())
    }

    async fn finish_transfer(
        &self,
        request_id: pab_protocol::RequestId,
        result: &Result<(), TaskServiceError>,
    ) -> Result<(), TaskServiceError> {
        let (state, message) = match result {
            Ok(()) => ("completed", None),
            Err(error) => ("failed", Some(error.to_string())),
        };
        self.store
            .finish_transfer(request_id, state, message.as_deref())
            .await?;
        Ok(())
    }

    async fn handle_request(
        &self,
        initiated_by: OperatorRef,
        request: DeviceTaskRequest,
    ) -> Result<DeviceTaskResponse, TaskServiceError> {
        match request {
            DeviceTaskRequest::SystemQuery {
                request_id, query, ..
            } => Ok(DeviceTaskResponse::SystemQuery {
                reply: Box::new(self.system_query(initiated_by, request_id, query).await?),
            }),
            DeviceTaskRequest::GetSystemQuery { request_id, .. } => {
                Ok(DeviceTaskResponse::SystemQuery {
                    reply: Box::new(self.get_system_query(initiated_by, request_id).await?),
                })
            }
            DeviceTaskRequest::CancelFileSystem { request_id, .. } => {
                Ok(DeviceTaskResponse::FileSystem {
                    reply: Box::new(self.cancel_hash(initiated_by, request_id).await?),
                })
            }
            DeviceTaskRequest::GetFileSystem { request_id, .. } => {
                Ok(DeviceTaskResponse::FileSystem {
                    reply: Box::new(self.lookup_filesystem(initiated_by, request_id).await?),
                })
            }
            DeviceTaskRequest::FileSystem { .. } => Err(TaskServiceError::InvalidRequest(
                "filesystem requests require their binary stream handler",
            )),
            DeviceTaskRequest::GetEnvironment { .. } => Ok(DeviceTaskResponse::Environment {
                filesystem_schema_version: Some(3),
                system_query_schema_version: Some(pab_protocol::SYSTEM_QUERY_SCHEMA_VERSION),
                screenshot_schema_version: Some(pab_protocol::SCREENSHOT_SCHEMA_VERSION),
                context: Box::new(TargetContext {
                    device_ref: self.device_ref,
                    execution: self.execution_context.clone(),
                    source: TargetContextSource::ExecutorVerified,
                    observed_at_unix_ms: unix_millis()?,
                    freshness: ContextFreshness::Current,
                }),
            }),
            DeviceTaskRequest::GetPresence { .. } => Ok(DeviceTaskResponse::Presence {
                active_operators: self.active_sessions.operator_count(),
            }),
            DeviceTaskRequest::SubmitCommand {
                request_id,
                command,
                ..
            } => {
                let snapshot = self
                    .submit_command(initiated_by, request_id, command)
                    .await?;
                Ok(DeviceTaskResponse::Submitted {
                    snapshot: Box::new(snapshot),
                })
            }
            DeviceTaskRequest::GetTask { task_ref, .. } => {
                self.validate_task_ref(task_ref)?;
                let snapshot = self.store.get_task(initiated_by, task_ref).await?;
                Ok(DeviceTaskResponse::Snapshot {
                    snapshot: Box::new(snapshot),
                })
            }
            DeviceTaskRequest::GetTransfer { request_id, .. } => {
                let snapshot = self.store.get_transfer(initiated_by, request_id).await?;
                Ok(DeviceTaskResponse::Transfer { snapshot })
            }
            DeviceTaskRequest::ListDirectory {
                request_id,
                path,
                after,
                limit,
                ..
            } => {
                self.store
                    .start_directory_read(request_id, initiated_by, &path)
                    .await?;
                let result = directory::list(request_id, path, after, limit).await;
                let (state, count, message) = match &result {
                    Ok(page) => ("completed", page.entries.len(), None),
                    Err(error) => ("failed", 0, Some(error.to_string())),
                };
                self.store
                    .finish_directory_read(request_id, state, count, message.as_deref())
                    .await?;
                Ok(DeviceTaskResponse::Directory { page: result? })
            }
            DeviceTaskRequest::ListWindows { request_id, .. } => {
                self.store
                    .start_window_read(request_id, initiated_by)
                    .await?;
                let result = if cfg!(any(
                    target_os = "windows",
                    target_os = "linux",
                    target_os = "macos"
                )) {
                    crate::local_ipc::request_window_list()
                        .await
                        .map(|entries| WindowList {
                            request_id,
                            entries,
                        })
                        .map_err(TaskServiceError::WindowHelper)
                } else {
                    Err(TaskServiceError::Unsupported)
                };
                let (state, count, message) = match &result {
                    Ok(list) => ("completed", list.entries.len(), None),
                    Err(error) => ("failed", 0, Some(error.to_string())),
                };
                self.store
                    .finish_window_read(request_id, state, count, message.as_deref())
                    .await?;
                Ok(DeviceTaskResponse::Windows { list: result? })
            }
            DeviceTaskRequest::DesktopInput {
                request_id, event, ..
            } => {
                if !cfg!(windows) {
                    return Err(TaskServiceError::Unsupported);
                }
                let kind = match event {
                    DesktopInputEvent::MouseMove { .. } => "mouse_move",
                    DesktopInputEvent::MouseButton { .. } => "mouse_button",
                    DesktopInputEvent::MouseWheel { .. } => "mouse_wheel",
                    DesktopInputEvent::Key { .. } => "key",
                    DesktopInputEvent::SecureAttention => "secure_attention",
                };
                self.store
                    .start_desktop_input(request_id, initiated_by, kind)
                    .await?;
                let result = if matches!(event, DesktopInputEvent::SecureAttention) {
                    crate::windows_sas::send_secure_attention()
                        .map_err(TaskServiceError::SecureAttention)
                } else {
                    crate::local_ipc::request_desktop_input(event)
                        .await
                        .map_err(TaskServiceError::WindowHelper)
                };
                let (state, message) = match &result {
                    Ok(()) => ("completed", None),
                    Err(error) => ("failed", Some(error.to_string())),
                };
                self.store
                    .finish_directory_read(request_id, state, 1, message.as_deref())
                    .await?;
                result?;
                Ok(DeviceTaskResponse::DesktopInputApplied { request_id })
            }
            DeviceTaskRequest::CaptureScreenshot { .. }
            | DeviceTaskRequest::CaptureScreenshotV2 { .. } => {
                unreachable!("handled before dispatch")
            }
            DeviceTaskRequest::OpenTerminal { .. }
            | DeviceTaskRequest::TerminalInput { .. }
            | DeviceTaskRequest::TerminalRead { .. }
            | DeviceTaskRequest::TerminalResize { .. }
            | DeviceTaskRequest::TerminalClose { .. } => unreachable!("handled before dispatch"),
            DeviceTaskRequest::ReadOutput {
                task_ref,
                stream,
                offset,
                max_bytes,
                ..
            } => {
                self.validate_task_ref(task_ref)?;
                if max_bytes == 0 || max_bytes > MAX_OUTPUT_READ_BYTES {
                    return Err(TaskServiceError::InvalidRequest(
                        "output read size is outside the supported range",
                    ));
                }
                let (chunk, range) = self
                    .store
                    .read_output(initiated_by, task_ref, stream, offset, max_bytes)
                    .await?;
                Ok(DeviceTaskResponse::Output { chunk, range })
            }
            DeviceTaskRequest::Cancel {
                task_ref, reason, ..
            } => {
                let snapshot = self.cancel(initiated_by, task_ref, reason).await?;
                Ok(DeviceTaskResponse::CancelAccepted {
                    snapshot: Box::new(snapshot),
                })
            }
            DeviceTaskRequest::Subscribe { .. }
            | DeviceTaskRequest::UploadFile { .. }
            | DeviceTaskRequest::DownloadFile { .. } => unreachable!("handled before dispatch"),
        }
    }

    async fn submit_command(
        &self,
        initiated_by: OperatorRef,
        request_id: pab_protocol::RequestId,
        command: CommandTaskSpec,
    ) -> Result<TaskSnapshot, TaskServiceError> {
        validate_command(&command)?;
        if !self
            .execution_context
            .matches_expected(&command.expected_environment)
        {
            return Err(TaskServiceError::EnvironmentChanged);
        }
        let mut execution_context = self.execution_context.clone();
        execution_context.cwd.clone_from(&command.cwd);
        let outcome = self
            .store
            .accept_command(
                self.device_ref,
                initiated_by,
                request_id,
                &command,
                execution_context,
                unix_millis()?,
            )
            .await?;
        match outcome {
            AcceptTaskOutcome::Existing(snapshot) => Ok(snapshot),
            AcceptTaskOutcome::Created(snapshot) => {
                let task_ref = snapshot.task_ref;
                let (cancel, receiver) = watch::channel(None);
                let mut active = self.active.lock().await;
                if active.len() >= MAX_ACTIVE_TASKS {
                    drop(active);
                    self.finish_outputs(task_ref).await?;
                    return Ok(self
                        .store
                        .record_event(
                            task_ref,
                            TaskEventKind::Failed {
                                error: TaskError {
                                    code: "executor_busy".to_owned(),
                                    message: "Executor has too many active command tasks"
                                        .to_owned(),
                                    exit_code: None,
                                },
                            },
                            unix_millis()?,
                        )
                        .await?);
                }
                active.insert(task_ref.task_id, cancel);
                drop(active);
                let service = self.clone();
                tokio::spawn(async move {
                    if let Err(error) = service.run_command(task_ref, command, receiver).await {
                        tracing::error!(task_id = %task_ref.task_id, %error, "task failed");
                        if let Err(finalize_error) = service.finalize_worker_error(task_ref).await {
                            tracing::error!(
                                task_id = %task_ref.task_id,
                                %finalize_error,
                                "task finalization failed"
                            );
                        }
                    }
                    service.active.lock().await.remove(&task_ref.task_id);
                });
                Ok(snapshot)
            }
        }
    }

    async fn cancel(
        &self,
        initiated_by: OperatorRef,
        task_ref: TaskRef,
        reason: String,
    ) -> Result<TaskSnapshot, TaskServiceError> {
        self.validate_task_ref(task_ref)?;
        if reason.trim().is_empty() || reason.len() > MAX_CANCEL_REASON_BYTES {
            return Err(TaskServiceError::InvalidRequest(
                "cancel reason is empty or too long",
            ));
        }
        let current = self.store.get_task(initiated_by, task_ref).await?;
        if current.state.is_terminal() || current.state == pab_protocol::TaskState::CancelRequested
        {
            return Err(TaskServiceError::NotCancellable);
        }
        let sender = self
            .active
            .lock()
            .await
            .get(&task_ref.task_id)
            .cloned()
            .ok_or(TaskServiceError::NotCancellable)?;
        let snapshot = self
            .store
            .record_event(task_ref, TaskEventKind::CancelRequested, unix_millis()?)
            .await?;
        sender
            .send(Some(reason))
            .map_err(|_| TaskServiceError::NotCancellable)?;
        Ok(snapshot)
    }

    fn validate_task_ref(&self, task_ref: TaskRef) -> Result<(), TaskServiceError> {
        if task_ref.device_ref != self.device_ref {
            return Err(TaskServiceError::NotFound);
        }
        Ok(())
    }
}

fn validate_command(command: &CommandTaskSpec) -> Result<(), TaskServiceError> {
    if command.program.trim().is_empty() || command.program.len() > MAX_COMMAND_PROGRAM_BYTES {
        return Err(TaskServiceError::InvalidRequest(
            "program is empty or too long",
        ));
    }
    if command.args.len() > MAX_COMMAND_ARGUMENTS
        || command
            .args
            .iter()
            .any(|argument| argument.len() > MAX_COMMAND_ARGUMENT_BYTES)
    {
        return Err(TaskServiceError::InvalidRequest(
            "command arguments exceed the supported limits",
        ));
    }
    if command.display_summary.trim().is_empty()
        || command.display_summary.len() > MAX_DISPLAY_SUMMARY_BYTES
    {
        return Err(TaskServiceError::InvalidRequest(
            "display summary is empty or too long",
        ));
    }
    if command
        .cwd
        .as_ref()
        .is_some_and(|cwd| cwd.len() > MAX_CWD_BYTES)
    {
        return Err(TaskServiceError::InvalidRequest(
            "working directory is too long",
        ));
    }
    Ok(())
}

fn error_response(error: &TaskServiceError) -> DeviceTaskResponse {
    let code = match error {
        TaskServiceError::InvalidRequest(_) => DeviceTaskErrorCode::InvalidRequest,
        TaskServiceError::DirectoryRead(error) if error.kind() == std::io::ErrorKind::NotFound => {
            DeviceTaskErrorCode::NotFound
        }
        TaskServiceError::DirectoryRead(error)
            if error.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            DeviceTaskErrorCode::AccessDenied
        }
        TaskServiceError::EnvironmentChanged => DeviceTaskErrorCode::EnvironmentChanged,
        TaskServiceError::AccessDenied => DeviceTaskErrorCode::AccessDenied,
        TaskServiceError::NotFound | TaskServiceError::Store(TaskStoreError::NotFound) => {
            DeviceTaskErrorCode::NotFound
        }
        TaskServiceError::Store(TaskStoreError::RequestConflict) => {
            DeviceTaskErrorCode::RequestConflict
        }
        TaskServiceError::Store(TaskStoreError::InvalidOutputOffset)
        | TaskServiceError::Store(TaskStoreError::OffsetTooLarge) => {
            DeviceTaskErrorCode::InvalidRequest
        }
        TaskServiceError::Store(TaskStoreError::Runtime(
            TaskRuntimeError::TerminalTask(_) | TaskRuntimeError::InvalidTransition { .. },
        )) => DeviceTaskErrorCode::NotCancellable,
        TaskServiceError::NotCancellable => DeviceTaskErrorCode::NotCancellable,
        TaskServiceError::Unsupported
        | TaskServiceError::WindowHelper(_)
        | TaskServiceError::SecureAttention(_) => DeviceTaskErrorCode::Unsupported,
        TaskServiceError::Store(_) => DeviceTaskErrorCode::StorageUnavailable,
        _ => DeviceTaskErrorCode::Internal,
    };
    let message = match code {
        DeviceTaskErrorCode::InvalidRequest => "invalid task request",
        DeviceTaskErrorCode::EnvironmentChanged => "target execution environment changed",
        DeviceTaskErrorCode::NotFound => "requested task or path was not found",
        DeviceTaskErrorCode::RequestConflict => {
            "request ID was already used with different arguments"
        }
        DeviceTaskErrorCode::NotCancellable => "task is no longer cancellable",
        DeviceTaskErrorCode::StorageUnavailable => "Executor task storage is unavailable",
        DeviceTaskErrorCode::Internal => "Executor task operation failed",
        DeviceTaskErrorCode::AccessDenied => "directory access denied",
        DeviceTaskErrorCode::Unsupported => {
            "remote desktop operation is unavailable on this device"
        }
    };
    DeviceTaskResponse::Error {
        code,
        message: message.to_owned(),
    }
}

async fn send_error(
    stream: &mut PabBiStream,
    timeout: Duration,
    code: DeviceTaskErrorCode,
    message: &str,
) -> Result<(), TaskServiceError> {
    stream
        .send_json(
            &DeviceTaskResponse::Error {
                code,
                message: message.to_owned(),
            },
            timeout,
        )
        .await?;
    Ok(())
}

fn unix_millis() -> Result<i64, TaskServiceError> {
    let value = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| TaskServiceError::Clock)?
        .as_millis();
    i64::try_from(value).map_err(|_| TaskServiceError::Clock)
}

#[derive(Debug, Error)]
pub(crate) enum TaskServiceError {
    #[error("invalid task request: {0}")]
    InvalidRequest(&'static str),
    #[error("the target execution environment changed before the task was accepted")]
    EnvironmentChanged,
    #[error("task was not found")]
    NotFound,
    #[error("terminal session belongs to a different operator")]
    AccessDenied,
    #[error("task is no longer cancellable")]
    NotCancellable,
    #[error("system time or output offset is outside the supported range")]
    Clock,
    #[error(transparent)]
    Store(#[from] TaskStoreError),
    #[error(transparent)]
    Connection(#[from] PabConnectionError),
    #[error("process operation failed: {0}")]
    Process(#[from] std::io::Error),
    #[error("directory read failed: {0}")]
    DirectoryRead(std::io::Error),
    #[error("task worker failed: {0}")]
    Worker(#[from] tokio::task::JoinError),
    #[error("window listing is unsupported on this device")]
    Unsupported,
    #[error("interactive window helper failed: {0}")]
    WindowHelper(crate::local_ipc::LocalIpcError),
    #[error("Windows secure attention failed: {0}")]
    SecureAttention(std::io::Error),
    #[error("terminal operation failed: {0}")]
    Terminal(#[from] pab_terminal::TerminalError),
}

impl TaskServiceError {
    pub(crate) const fn is_connection_end(&self) -> bool {
        matches!(self, Self::Connection(_))
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod transfer_tests;
