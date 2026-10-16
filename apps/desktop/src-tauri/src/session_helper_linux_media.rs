//! Linux capture follows the existing graphical user's lifecycle. Its media
//! socket is independent of GUI status, screenshots and application requests.
use pab_desktop_media::NativeBackend;
use pab_os_control::execution::{LinuxGraphicalSession, process_user_identity};
use std::time::Duration;

pub(crate) struct Task(tauri::async_runtime::JoinHandle<()>);
impl Task {
    pub(super) async fn shutdown(&mut self) {
        self.0.abort();
        let _ = (&mut self.0).await;
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
pub(crate) fn start() -> Result<Task, String> {
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
    );
    Ok(Task(tauri::async_runtime::spawn(async move {
        loop {
            if let Ok(Ok(session)) =
                tokio::task::spawn_blocking(move || LinuxGraphicalSession::for_process(pid, uid))
                    .await
            {
                let switched = async {
                    loop {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        if !session.current(pid) {
                            break;
                        }
                    }
                };
                tokio::select! {
                    result = pab_executor::local_ipc::media::serve_helper(&backend) => {
                        if let Err(error) = result { tracing::debug!(%error, "Linux media helper disconnected"); }
                    }
                    _ = switched => {}
                }
                // The process adapter's guard is released only after reaping
                // the previous capture, including a cancelled pending read.
                backend.idle().await;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })))
}
