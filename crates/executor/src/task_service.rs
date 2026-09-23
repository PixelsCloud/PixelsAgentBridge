use std::{
    collections::HashMap,
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime},
};

use pab_protocol::{
    CommandTaskSpec, ContextFreshness, DEVICE_TASK_SCHEMA_VERSION, DeviceRef, DeviceTaskErrorCode,
    DeviceTaskRequest, DeviceTaskResponse, ExecutionContext, MAX_COMMAND_ARGUMENT_BYTES,
    MAX_COMMAND_ARGUMENTS, MAX_COMMAND_PROGRAM_BYTES, MAX_OUTPUT_READ_BYTES, OperatorRef,
    OutputStream, TargetContext, TargetContextSource, TaskError, TaskEventKind, TaskId, TaskRef,
    TaskSnapshot,
};
use pab_task_runtime::TaskRuntimeError;
use pab_transport::{PabBiStream, PabConnectionError};
use thiserror::Error;
use tokio::sync::{Mutex, watch};

mod command;
mod subscription;

use crate::task_store::{AcceptTaskOutcome, TaskStore, TaskStoreError};

const MAX_DISPLAY_SUMMARY_BYTES: usize = 1024;
const MAX_CWD_BYTES: usize = 32 * 1024;
const MAX_CANCEL_REASON_BYTES: usize = 1024;
const MAX_ACTIVE_TASKS: usize = 32;

#[derive(Clone)]
pub(crate) struct TaskService {
    store: TaskStore,
    device_ref: DeviceRef,
    execution_context: ExecutionContext,
    active: Arc<Mutex<HashMap<TaskId, watch::Sender<Option<String>>>>>,
}

impl TaskService {
    pub async fn open(
        database_file: &Path,
        device_ref: DeviceRef,
        execution_context: ExecutionContext,
    ) -> Result<Self, TaskServiceError> {
        let store = TaskStore::open(database_file).await?;
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
        })
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

        let response = match self.handle_request(initiated_by, request).await {
            Ok(response) => response,
            Err(error) => error_response(&error),
        };
        stream.send_json(&response, timeout).await?;
        Ok(())
    }

    async fn handle_request(
        &self,
        initiated_by: OperatorRef,
        request: DeviceTaskRequest,
    ) -> Result<DeviceTaskResponse, TaskServiceError> {
        match request {
            DeviceTaskRequest::GetEnvironment { .. } => Ok(DeviceTaskResponse::Environment {
                context: Box::new(TargetContext {
                    device_ref: self.device_ref,
                    execution: self.execution_context.clone(),
                    source: TargetContextSource::ExecutorVerified,
                    observed_at_unix_ms: unix_millis()?,
                    freshness: ContextFreshness::Current,
                }),
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
            DeviceTaskRequest::Subscribe { .. } => unreachable!("handled before dispatch"),
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
        TaskServiceError::EnvironmentChanged => DeviceTaskErrorCode::EnvironmentChanged,
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
        TaskServiceError::Store(_) => DeviceTaskErrorCode::StorageUnavailable,
        _ => DeviceTaskErrorCode::Internal,
    };
    let message = match code {
        DeviceTaskErrorCode::InvalidRequest => "invalid task request",
        DeviceTaskErrorCode::EnvironmentChanged => "target execution environment changed",
        DeviceTaskErrorCode::NotFound => "task was not found",
        DeviceTaskErrorCode::RequestConflict => {
            "request ID was already used with different arguments"
        }
        DeviceTaskErrorCode::NotCancellable => "task is no longer cancellable",
        DeviceTaskErrorCode::StorageUnavailable => "Executor task storage is unavailable",
        DeviceTaskErrorCode::Internal => "Executor task operation failed",
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
    #[error("task worker failed: {0}")]
    Worker(#[from] tokio::task::JoinError),
}

impl TaskServiceError {
    pub(crate) const fn is_connection_end(&self) -> bool {
        matches!(self, Self::Connection(_))
    }
}

#[cfg(test)]
mod tests;
