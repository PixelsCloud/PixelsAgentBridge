//! Shared bounded command execution. Storage and caller authorization belong to the host.
use pab_protocol::{CommandTaskSpec, OutputStream, TaskCompletion, TaskError, TaskEventKind};
use std::{future::Future, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::watch,
};
const OUTPUT_CHUNK_BYTES: usize = 8 * 1024;

/// The sink applies backpressure; a persistence failure ends execution without replay.
pub trait CommandSink: Clone + Send + Sync + 'static {
    type Error: From<std::io::Error> + Send + 'static;
    fn event(&self, event: TaskEventKind) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn output(
        &self,
        stream: OutputStream,
        bytes: &[u8],
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn finish_outputs(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
pub async fn execute<S: CommandSink>(
    sink: S,
    command: CommandTaskSpec,
    mut cancel: watch::Receiver<Option<String>>,
) -> Result<(), S::Error> {
    let prior_cancel = cancel.borrow().clone();
    if let Some(reason) = prior_cancel {
        sink.finish_outputs().await?;
        return sink.event(TaskEventKind::Cancelled { reason }).await;
    }
    sink.event(TaskEventKind::Running).await?;
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
    #[cfg(target_os = "android")]
    {
        process
            .env_clear()
            .envs(&command.options.env)
            .process_group(0);
    }
    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(error) => {
            sink.finish_outputs().await?;
            sink.event(TaskEventKind::Failed {
                error: TaskError {
                    code: "process_start_failed".to_owned(),
                    message: error.to_string(),
                    exit_code: None,
                },
            })
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
    let mut stdout_task = tokio::spawn(drain_output(sink.clone(), OutputStream::Stdout, stdout));
    let mut stderr_task = tokio::spawn(drain_output(sink.clone(), OutputStream::Stderr, stderr));

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
        result = wait_process(&mut child) => result,
        _ = deadline => {
            timed_out = true;
            #[cfg(target_os = "android")]
            stop_group(&child);
            child.kill().await?;
            child.wait().await
        },
        changed = cancel.changed() => {
            if changed.is_ok() {
                cancelled = cancel.borrow().clone();
                #[cfg(target_os = "android")]
            stop_group(&child);
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
            Ok(result) => result.map_err(std::io::Error::other)??,
            Err(_) => {
                output_incomplete = true;
                task.abort();
                let _ = task.await;
            }
        }
    }
    sink.finish_outputs().await?;

    let event = if timed_out {
        TaskEventKind::Failed {
            error: TaskError {
                code: "execution_timeout".into(),
                message: if cfg!(target_os = "android") {
                    "Command deadline exceeded; the command process group was stopped; processes that create independent sessions are outside this cleanup boundary.".into()
                } else {
                    "Command deadline exceeded; the direct child process was stopped. Descendant processes may remain.".into()
                },
                exit_code: status.code(),
            },
        }
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
    sink.event(event).await?;
    Ok(())
}

async fn drain_output<S: CommandSink>(
    sink: S,
    stream: OutputStream,
    mut reader: impl AsyncRead + Unpin,
) -> Result<(), S::Error> {
    let mut buffer = vec![0_u8; OUTPUT_CHUNK_BYTES];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        sink.output(stream, &buffer[..read]).await?;
    }
}

/// On Android observe exit without reaping, so the owned process-group ID
/// cannot be recycled before cleanup. Desktop keeps its original wait behavior.
async fn wait_process(
    child: &mut tokio::process::Child,
) -> std::io::Result<std::process::ExitStatus> {
    #[cfg(target_os = "android")]
    if let Some(pid) = child.id() {
        loop {
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid,
                    &mut info,
                    libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
                )
            };
            if result < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error);
                }
            } else if unsafe { info.si_pid() } != 0 {
                stop_group(child);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    child.wait().await
}
#[cfg(target_os = "android")]
fn stop_group(child: &tokio::process::Child) {
    if let Some(pid) = child.id() {
        // The leader is still unreaped. Only this command's process group.
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
}
