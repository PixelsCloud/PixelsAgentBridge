use std::{process::Stdio, time::Duration};

use pab_protocol::{
    CommandTaskSpec, OutputStream, TaskCompletion, TaskError, TaskEventKind, TaskRef,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
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
            .envs(&command.options.env)
            .kill_on_drop(true)
            .stdin(if command.options.stdin_text.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
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
        let stdin_task = command.options.stdin_text.map(|text| {
            let mut stdin = child.stdin.take().expect("stdin was piped");
            tokio::spawn(async move {
                stdin.write_all(text.as_bytes()).await?;
                stdin.shutdown().await
            })
        });
        let mut stdout_task = tokio::spawn(drain_output(
            self.store.clone(),
            task_ref,
            OutputStream::Stdout,
            stdout,
        ));
        let mut stderr_task = tokio::spawn(drain_output(
            self.store.clone(),
            task_ref,
            OutputStream::Stderr,
            stderr,
        ));

        let mut cancelled = None;
        let mut timed_out = false;
        let deadline = async {
            match command.options.timeout_ms {
                Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                None => std::future::pending::<()>().await,
            }
        };
        let status = tokio::select! {
            biased;
            result = child.wait() => result,
            _ = deadline => {
                timed_out = true;
                child.kill().await?;
                child.wait().await
            },
            changed = cancel.changed() => {
                if changed.is_ok() {
                    cancelled = cancel.borrow().clone();
                    child.kill().await?;
                }
                child.wait().await
            }
        };
        let status = status?;
        let mut input_failed = false;
        if let Some(mut task) = stdin_task {
            match tokio::time::timeout(Duration::from_secs(1), &mut task).await {
                Ok(result) => {
                    input_failed = !matches!(result, Ok(Ok(())));
                }
                Err(_) => {
                    input_failed = true;
                    task.abort();
                    let _ = task.await;
                }
            }
        }
        let mut output_incomplete = false;
        for task in [&mut stdout_task, &mut stderr_task] {
            match tokio::time::timeout(Duration::from_secs(3), &mut *task).await {
                Ok(result) => result??,
                Err(_) => {
                    output_incomplete = true;
                    task.abort();
                    let _ = task.await;
                }
            }
        }
        self.finish_outputs(task_ref).await?;

        let event = if timed_out {
            TaskEventKind::Failed { error: TaskError {
                code: "execution_timeout".into(),
                message: "Command deadline exceeded; the direct child process was stopped. Descendant processes may remain.".into(),
                exit_code: status.code(),
            }}
        } else if let Some(reason) = cancelled {
            TaskEventKind::Cancelled { reason }
        } else if input_failed || output_incomplete {
            TaskEventKind::Failed { error: TaskError {
                code: if input_failed { "stdin_write_failed" } else { "output_drain_timeout" }.into(),
                message: "The process exited, but input delivery or complete output capture could not be confirmed".into(),
                exit_code: status.code(),
            }}
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
