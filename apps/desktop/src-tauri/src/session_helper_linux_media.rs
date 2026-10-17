//! Linux capture follows the existing graphical user's lifecycle. Its media
//! socket is independent of GUI status, screenshots and application requests.
use pab_desktop_media::NativeBackend;
use pab_os_control::execution::{LinuxGraphicalSession, process_user_identity};
use std::time::Duration;
use tokio::sync::oneshot;

pub(crate) struct Task {
    task: tauri::async_runtime::JoinHandle<()>,
    stop: Option<oneshot::Sender<()>>,
}
impl Task {
    pub(super) async fn shutdown(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = (&mut self.task).await;
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub(crate) fn start() -> Result<Task, String> {
    start_inner(false)
}
/// A headless login-screen helper registers media only. GUI and application
/// helpers retain the user-only observation above.
pub(crate) fn start_media_only() -> Result<Task, String> {
    start_inner(true)
}
fn start_inner(media_only: bool) -> Result<Task, String> {
    let pid = std::process::id();
    let identity = process_user_identity(pid).map_err(|e| e.to_string())?;
    let uid = identity
        .account_id
        .strip_prefix("uid:")
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|uid| *uid != 0)
        .ok_or("Linux media requires a graphical user process")?;
    let backend = NativeBackend::new(
        std::env::current_exe().map_err(|e| e.to_string())?,
        vec!["--media-worker".into()],
    )
    .with_input();
    let (stop, mut stopped) = oneshot::channel();
    let task = tauri::async_runtime::spawn(async move {
        loop {
            if !matches!(stopped.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
                break;
            }
            if let Ok(Ok(session)) = tokio::task::spawn_blocking(move || {
                if media_only {
                    LinuxGraphicalSession::for_media_process(pid, uid)
                } else {
                    LinuxGraphicalSession::for_process(pid, uid)
                }
            })
            .await
            {
                // Recover a prior process's persisted X11 releases before
                // offering media. A live owner's file lock defers recovery.
                if let Err(error) = backend.recover_input().await {
                    tracing::debug!(%error, "Linux input recovery deferred");
                    tokio::select! {
                        _ = &mut stopped => break,
                        _ = tokio::time::sleep(Duration::from_millis(500)) => {},
                    }
                    continue;
                }
                let switched = async {
                    loop {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        if !session.current(pid) {
                            break;
                        }
                    }
                };
                let serve = async {
                    if media_only {
                        pab_executor::local_ipc::media::serve_supervised_helper(&backend).await
                    } else {
                        pab_executor::local_ipc::media::serve_helper(&backend).await
                    }
                };
                tokio::select! {
                    result = serve => {
                        if let Err(error) = result { tracing::debug!(%error, "Linux media helper disconnected"); }
                    }
                    _ = switched => {}
                    _ = &mut stopped => break,
                }
                // The process adapter's guard is released only after reaping
                // the previous capture, including a cancelled pending read.
                backend.idle().await;
            }
            tokio::select! {
                _ = &mut stopped => break,
                _ = tokio::time::sleep(Duration::from_millis(500)) => {},
            }
        }
        // Dropping the serving future closes native pipes. Input release and
        // recovery run in a separate watchdog, so keep the helper alive until
        // both gates drain before its supervisor reaps the process group.
        // Startup recovery is also allowed to finish instead of being aborted.
        backend.idle().await;
    });
    Ok(Task {
        task,
        stop: Some(stop),
    })
}
