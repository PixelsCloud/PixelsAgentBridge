//! Media follows the existing Aqua/LoginWindow helper lifecycle, over separate
//! sockets. No GUI or browser input engine is reused for a remote-view lease.
use pab_desktop_media::NativeBackend;
use pab_protocol::{DesktopErrorCode, DesktopScreen, DesktopVideoConfig, RemoteDesktopInput};
use pab_remote_desktop::{
    DesktopBackend, DesktopError, DesktopInputSink, DesktopSource, MediaFuture,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, OwnedMutexGuard};

fn error(value: impl std::fmt::Display) -> DesktopError {
    DesktopError::Rejected {
        code: DesktopErrorCode::MediaUnavailable,
        message: value.to_string(),
    }
}

struct Backend {
    media: NativeBackend,
    input: Arc<Mutex<()>>,
}
struct Input {
    session: pab_desktop_control::MacMediaInput,
    // Fields drop in order: release native input on AppKit before allowing the
    // next generation to acquire its input slot.
    _permit: OwnedMutexGuard<()>,
}
impl DesktopInputSink for Input {
    fn apply(&mut self, event: RemoteDesktopInput) -> MediaFuture<'_, ()> {
        let operation = self.session.operation(event);
        Box::pin(async move {
            tokio::task::spawn_blocking(operation)
                .await
                .map_err(error)?
                .map_err(error)
        })
    }
}
impl DesktopBackend for Backend {
    fn supports_input(&self) -> bool {
        true
    }
    fn input(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopInputSink>> {
        Box::pin(async move {
            config.validate().map_err(error)?;
            let permit = self.input.clone().lock_owned().await;
            let session = tokio::task::spawn_blocking(move || {
                pab_desktop_control::MacMediaInput::new(config.screen)
            })
            .await
            .map_err(error)?
            .map_err(error)?;
            Ok(Box::new(Input {
                session,
                _permit: permit,
            }) as Box<dyn DesktopInputSink>)
        })
    }
    fn screens(&self) -> MediaFuture<'_, Vec<DesktopScreen>> {
        Box::pin(async {
            // Wake only when needed, with the existing bounded read-side
            // preparation. No keyboard/mouse event is used to wake a display.
            tokio::task::spawn_blocking(|| {
                pab_desktop_control::select_macos_monitor(None).map(|_| ())
            })
            .await
            .map_err(error)?
            .map_err(error)?;
            self.media.screens().await
        })
    }
    fn start(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopSource>> {
        Box::pin(async move {
            pab_desktop_control::require_screen_capture().map_err(error)?;
            self.media.start(config).await
        })
    }
}

pub(super) struct Task(tauri::async_runtime::JoinHandle<()>);
impl Task {
    pub(super) async fn shutdown(&mut self) {
        self.0.abort();
        // Await cancellation while the AppKit loop is still running. Dropping
        // an active Input synchronously dispatches its release to that loop.
        let _ = (&mut self.0).await;
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
pub(super) fn start(desktop: Option<String>) -> Result<Task, String> {
    let backend = Backend {
        media: NativeBackend::new(
            std::env::current_exe().map_err(|e| e.to_string())?,
            vec!["--media-worker".into()],
        ),
        input: Arc::new(Mutex::new(())),
    };
    Ok(Task(tauri::async_runtime::spawn(async move {
        loop {
            if super::desktop_is_active(desktop.as_deref()) {
                let recovery =
                    tokio::task::spawn_blocking(pab_desktop_control::recover_abandoned_input).await;
                if !matches!(recovery, Ok(Ok(()))) {
                    tracing::debug!("waiting for macOS input release recovery");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                let switched = async {
                    loop {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        if !super::desktop_is_active(desktop.as_deref()) {
                            break;
                        }
                    }
                };
                tokio::select! {
                    result = pab_executor::local_ipc::media::serve_helper(&backend) => {
                        if let Err(error) = result { tracing::debug!(%error, "Mac media helper disconnected"); }
                    }
                    _ = switched => {}
                }
                backend.media.idle().await;
                drop(backend.input.lock().await);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })))
}
