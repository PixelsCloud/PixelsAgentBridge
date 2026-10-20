//! Experiment-only compressed-pipe adapter. Not enabled in the installed app.
use pab_protocol::*;
use pab_remote_desktop::*;
use std::{path::PathBuf, process::Stdio, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{Mutex, OwnedMutexGuard},
};
const MAGIC: u32 = 0x31424150;
type AnyError = Box<dyn std::error::Error + Send + Sync>;
fn media_error(error: impl std::fmt::Display) -> DesktopError {
    DesktopError::Rejected {
        code: DesktopErrorCode::MediaUnavailable,
        message: error.to_string(),
    }
}

pub struct MediaChild {
    child: Option<Child>,
    input: ChildStdin,
    output: ChildStdout,
    permit: Option<OwnedMutexGuard<()>>,
}
impl MediaChild {
    fn spawn(
        path: &PathBuf,
        args: &[String],
        permit: Option<OwnedMutexGuard<()>>,
    ) -> std::io::Result<Self> {
        let mut command = Command::new(path);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000); // No console; the render window is created explicitly.
        let mut child = command.spawn()?;
        Ok(Self {
            input: child.stdin.take().unwrap(),
            output: child.stdout.take().unwrap(),
            child: Some(child),
            permit,
        })
    }
    async fn ready(&mut self) -> std::io::Result<()> {
        if self.output.read_u32_le().await? != MAGIC {
            return Err(std::io::Error::other("media IPC version mismatch"));
        }
        Ok(())
    }
}
impl Drop for MediaChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
            let permit = self.permit.take();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = child.wait().await;
                    // A subsequent source cannot start until the old process is
                    // reaped, even when cancellation dropped its pending pipe IO.
                    drop(permit);
                });
            }
        }
    }
}
pub struct NativeBackend {
    pub executable: PathBuf,
    pub mode: String,
    gate: Arc<Mutex<()>>,
}
impl NativeBackend {
    pub fn new(executable: PathBuf, mode: String) -> Self {
        Self {
            executable,
            mode,
            gate: Arc::new(Mutex::new(())),
        }
    }
    pub async fn idle(&self) {
        drop(self.gate.lock().await);
    }
}
struct NativeSource {
    process: MediaChild,
    index: u64,
}
impl DesktopSource for NativeSource {
    fn next_frame(&mut self) -> MediaFuture<'_, MediaFrame> {
        Box::pin(async {
            let operation = async {
                self.process.input.write_u8(1).await?;
                let size = self.process.output.read_u32_le().await? as usize;
                if size == 0 || size > DESKTOP_MAX_FRAME_BYTES {
                    return Err(std::io::Error::other("invalid media length"));
                }
                let index = self.process.output.read_u64_le().await?;
                if index != self.index {
                    return Err(std::io::Error::other("media sequence mismatch"));
                }
                let capture_time_us = self.process.output.read_u64_le().await?;
                let key = self.process.output.read_u32_le().await?;
                if key > 1 || (index == 0 && key == 0) {
                    return Err(std::io::Error::other("media keyframe mismatch"));
                }
                let mut bytes = vec![0; size];
                self.process.output.read_exact(&mut bytes).await?;
                self.index += 1;
                Ok(MediaFrame {
                    bytes,
                    capture_time_us,
                    keyframe: key == 1,
                })
            };
            operation.await.map_err(media_error)
        })
    }
}
impl DesktopBackend for NativeBackend {
    fn screens(&self) -> MediaFuture<'_, Vec<DesktopScreen>> {
        Box::pin(async {
            let mut process = MediaChild::spawn(&self.executable, &["inventory".into()], None)
                .map_err(media_error)?;
            let mut bytes = Vec::new();
            (&mut process.output)
                .take((DESKTOP_CONTROL_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .await
                .map_err(media_error)?;
            if bytes.len() > DESKTOP_CONTROL_BYTES {
                return Err(media_error("inventory exceeds limit"));
            }
            if !process
                .child
                .as_mut()
                .unwrap()
                .wait()
                .await
                .map_err(media_error)?
                .success()
            {
                return Err(media_error("screen inventory failed"));
            }
            let screens: Vec<DesktopScreen> =
                serde_json::from_slice(&bytes).map_err(media_error)?;
            validate_desktop_screens(&screens).map_err(media_error)?;
            Ok(screens)
        })
    }
    fn start(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopSource>> {
        Box::pin(async move {
            let permit = self.gate.clone().lock_owned().await;
            let mut process = MediaChild::spawn(
                &self.executable,
                &[
                    "capture".into(),
                    config.screen.id.clone(),
                    config.fps.to_string(),
                    self.mode.clone(),
                ],
                Some(permit),
            )
            .map_err(media_error)?;
            process.ready().await.map_err(media_error)?;
            let width = process.output.read_u32_le().await.map_err(media_error)?;
            let height = process.output.read_u32_le().await.map_err(media_error)?;
            if (width, height) != (config.screen.pixel_width, config.screen.pixel_height) {
                return Err(media_error("screen changed during source startup"));
            }
            Ok(Box::new(NativeSource { process, index: 0 }) as Box<dyn DesktopSource>)
        })
    }
}
pub struct NativeViewer {
    process: MediaChild,
}
impl NativeViewer {
    pub async fn open(
        path: &PathBuf,
        config: &DesktopVideoConfig,
        verify: bool,
    ) -> std::result::Result<Self, AnyError> {
        if config.screen.rotation != DesktopRotation::Identity {
            return Err("rotated native rendering not yet validated".into());
        }
        let mut process = MediaChild::spawn(
            path,
            &[
                "render".into(),
                config.screen.pixel_width.to_string(),
                config.screen.pixel_height.to_string(),
                config.fps.to_string(),
                if verify { "verify" } else { "normal" }.into(),
            ],
            None,
        )?;
        process.ready().await?;
        Ok(Self { process })
    }
    pub async fn present(&mut self, frame: &VideoFrame) -> std::result::Result<(), AnyError> {
        self.process
            .input
            .write_u32_le(frame.bytes.len().try_into()?)
            .await?;
        self.process
            .input
            .write_u64_le(frame.header.frame_id - 1)
            .await?;
        self.process.input.write_all(&frame.bytes).await?;
        let presented = self.process.output.read_u64_le().await?;
        if presented != frame.header.frame_id - 1 {
            return Err("native display confirmation mismatch".into());
        }
        Ok(())
    }
    pub async fn finish(mut self) -> std::result::Result<serde_json::Value, AnyError> {
        self.process.input.write_u32_le(0).await?;
        let displayed = self.process.output.read_u64_le().await?;
        let pixel_checks = self.process.output.read_u64_le().await?;
        let max_rgb_error = self.process.output.read_u32_le().await?;
        if !self.process.child.as_mut().unwrap().wait().await?.success() {
            return Err("native viewer failed on shutdown".into());
        }
        Ok(
            serde_json::json!({"displayed":displayed,"test_pixel_checks":pixel_checks,"test_max_rgb_error":max_rgb_error}),
        )
    }
}
