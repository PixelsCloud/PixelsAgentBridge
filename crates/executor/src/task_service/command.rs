use std::process::Stdio;

use pab_protocol::{
    CommandTaskSpec, OutputStream, TaskCompletion, TaskError, TaskEventKind, TaskRef,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::watch,
};

use super::{TaskService, TaskServiceError, unix_millis};
use crate::task_store::{TaskStore, TaskStoreError};

const OUTPUT_CHUNK_BYTES: usize = 8 * 1024;

impl TaskService {
    pub(super) async fn run_command(
        &self,
        task_ref: TaskRef,
        command: CommandTaskSpec,
        mut cancel: watch::Receiver<Option<String>>,
    ) -> Result<(), TaskServiceError> {
        self.store
            .record_event(task_ref, TaskEventKind::Running, unix_millis()?)
            .await?;
        let mut process = Command::new(&command.program);
        process
            .args(&command.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &command.cwd {
            process.current_dir(cwd);
        }
        let mut child = match process.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.finish_outputs(task_ref).await?;
                self.store
                    .record_event(
                        task_ref,
                        TaskEventKind::Failed {
                            error: TaskError {
                                code: "process_start_failed".to_owned(),
                                message: error.to_string(),
                                exit_code: None,
                            },
                        },
                        unix_millis()?,
                    )
                    .await?;
                return Ok(());
            }
        };
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        let stdout_task = tokio::spawn(drain_output(
            self.store.clone(),
            task_ref,
            OutputStream::Stdout,
            stdout,
        ));
        let stderr_task = tokio::spawn(drain_output(
            self.store.clone(),
            task_ref,
            OutputStream::Stderr,
            stderr,
        ));

        let mut cancelled = None;
        let status = tokio::select! {
            result = child.wait() => result,
            changed = cancel.changed() => {
                if changed.is_ok() {
                    cancelled = cancel.borrow().clone();
                    child.kill().await?;
                }
                child.wait().await
            }
        };
        let status = status?;
        stdout_task.await??;
        stderr_task.await??;
        self.finish_outputs(task_ref).await?;

        let event = if let Some(reason) = cancelled {
            TaskEventKind::Cancelled { reason }
        } else if status.success() {
            TaskEventKind::Succeeded {
                completion: TaskCompletion {
                    summary: "process exited successfully".to_owned(),
                    exit_code: status.code(),
                },
            }
        } else {
            TaskEventKind::Failed {
                error: TaskError {
                    code: "process_exit_failed".to_owned(),
                    message: match status.code() {
                        Some(code) => format!("process exited with code {code}"),
                        None => "process terminated without an exit code".to_owned(),
                    },
                    exit_code: status.code(),
                },
            }
        };
        self.store
            .record_event(task_ref, event, unix_millis()?)
            .await?;
        Ok(())
    }

    pub(super) async fn finalize_worker_error(
        &self,
        task_ref: TaskRef,
    ) -> Result<(), TaskServiceError> {
        self.finish_outputs(task_ref).await?;
        self.store
            .record_event(
                task_ref,
                TaskEventKind::Failed {
                    error: TaskError {
                        code: "executor_internal_error".to_owned(),
                        message: "Executor could not complete the command task".to_owned(),
                        exit_code: None,
                    },
                },
                unix_millis()?,
            )
            .await?;
        Ok(())
    }

    pub(super) async fn finish_outputs(&self, task_ref: TaskRef) -> Result<(), TaskServiceError> {
        self.store
            .complete_output(task_ref, OutputStream::Stdout)
            .await?;
        self.store
            .complete_output(task_ref, OutputStream::Stderr)
            .await?;
        Ok(())
    }
}

async fn drain_output(
    store: TaskStore,
    task_ref: TaskRef,
    stream: OutputStream,
    mut reader: impl AsyncRead + Unpin,
) -> Result<(), TaskStoreError> {
    let mut buffer = vec![0_u8; OUTPUT_CHUNK_BYTES];
    loop {
        let read = reader.read(&mut buffer).await.map_err(TaskStoreError::Io)?;
        if read == 0 {
            return Ok(());
        }
        store
            .append_output(task_ref, stream, &buffer[..read])
            .await?;
    }
}
