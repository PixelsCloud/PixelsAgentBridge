//! Bounded compressed-pipe adapter. Dropping any pending read retires the
//! entire worker, so partial packets can never be reused as a new frame.
#![forbid(unsafe_code)]
use pab_protocol::*;
use pab_remote_desktop::*;
use std::{path::PathBuf, process::Stdio, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
    sync::{Mutex, OwnedMutexGuard},
};
pub(crate) const MAGIC: u32 = 0x31424150;
type AnyError = Box<dyn std::error::Error + Send + Sync>;
pub(crate) fn media_error(error: impl std::fmt::Display) -> DesktopError {
    DesktopError::Rejected {
        code: DesktopErrorCode::MediaUnavailable,
        message: error.to_string(),
    }
}

pub struct MediaChild {
    child: Option<Child>,
    input: Arc<Mutex<ChildStdin>>,
    output: Option<ChildStdout>,
    events: Option<ChildStderr>,
    permit: Option<OwnedMutexGuard<()>>,
}
impl MediaChild {
    fn spawn(
        path: &PathBuf,
        prefix: &[String],
        args: &[String],
        permit: Option<OwnedMutexGuard<()>>,
    ) -> std::io::Result<Self> {
        let mut command = Command::new(path);
        let interactive = args.first().is_some_and(|s| s == "render")
            && args
                .get(4)
                .is_some_and(|s| s == "interactive" || s == "stream-interactive");
        command
            .args(prefix)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(if interactive {
                Stdio::piped()
            } else {
                Stdio::inherit()
            })
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000); // No console; the render window is created explicitly.
        let mut child = command.spawn()?;
        Ok(Self {
            input: Arc::new(Mutex::new(child.stdin.take().unwrap())),
            output: child.stdout.take(),
            events: child.stderr.take(),
            child: Some(child),
            permit,
        })
    }
    async fn ready(&mut self) -> std::io::Result<()> {
        if self.output.as_mut().unwrap().read_u32_le().await? != MAGIC {
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
    executable: PathBuf,
    prefix: Vec<String>,
    gate: Arc<Mutex<()>>,
    input_gate: Arc<Mutex<()>>,
    input_enabled: bool,
}
impl NativeBackend {
    pub fn new(executable: PathBuf, prefix: Vec<String>) -> Self {
        Self {
            executable,
            prefix,
            gate: Arc::new(Mutex::new(())),
            input_gate: Arc::new(Mutex::new(())),
            input_enabled: false,
        }
    }
    pub async fn idle(&self) {
        drop(self.gate.lock().await);
        drop(self.input_gate.lock().await);
    }
    /// Only the dedicated interactive media helper enables OS input. Capture
    /// experiments and all other NativeBackend callers remain view-only.
    pub fn with_input(mut self) -> Self {
        self.input_enabled = cfg!(windows);
        self
    }
    pub async fn recover_input(&self) -> std::io::Result<()> {
        let _permit = self.input_gate.lock().await;
        crate::input::recover(&self.executable, &self.prefix).await
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
                self.process.input.lock().await.write_u8(1).await?;
                let size = self.process.output.as_mut().unwrap().read_u32_le().await? as usize;
                if size == 0 || size > DESKTOP_MAX_FRAME_BYTES {
                    return Err(std::io::Error::other("invalid media length"));
                }
                let index = self.process.output.as_mut().unwrap().read_u64_le().await?;
                if index != self.index {
                    return Err(std::io::Error::other("media sequence mismatch"));
                }
                let capture_time_us = self.process.output.as_mut().unwrap().read_u64_le().await?;
                let key = self.process.output.as_mut().unwrap().read_u32_le().await?;
                if key > 1 || (index == 0 && key == 0) {
                    return Err(std::io::Error::other("media keyframe mismatch"));
                }
                let mut bytes = vec![0; size];
                self.process
                    .output
                    .as_mut()
                    .unwrap()
                    .read_exact(&mut bytes)
                    .await?;
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
    fn supports_input(&self) -> bool {
        self.input_enabled
    }
    fn input(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopInputSink>> {
        Box::pin(async move {
            if !self.input_enabled {
                return Err(media_error("native input is disabled"));
            }
            config.validate().map_err(media_error)?;
            let permit = self.input_gate.clone().lock_owned().await;
            let sink = crate::input::NativeInput::open(
                self.executable.clone(),
                self.prefix.clone(),
                config,
                permit,
            )
            .await
            .map_err(media_error)?;
            Ok(Box::new(sink) as Box<dyn DesktopInputSink>)
        })
    }
    fn screens(&self) -> MediaFuture<'_, Vec<DesktopScreen>> {
        Box::pin(async {
            let mut process =
                MediaChild::spawn(&self.executable, &self.prefix, &["inventory".into()], None)
                    .map_err(media_error)?;
            let mut bytes = Vec::new();
            process
                .output
                .as_mut()
                .unwrap()
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
            config.validate().map_err(media_error)?;
            let permit = self.gate.clone().lock_owned().await;
            let mut process = MediaChild::spawn(
                &self.executable,
                &self.prefix,
                &[
                    "capture".into(),
                    config.screen.id.clone(),
                    config.fps.to_string(),
                    "desktop".into(),
                ],
                Some(permit),
            )
            .map_err(media_error)?;
            process.ready().await.map_err(media_error)?;
            let width = process
                .output
                .as_mut()
                .unwrap()
                .read_u32_le()
                .await
                .map_err(media_error)?;
            let height = process
                .output
                .as_mut()
                .unwrap()
                .read_u32_le()
                .await
                .map_err(media_error)?;
            if (width, height) != (config.screen.pixel_width, config.screen.pixel_height) {
                return Err(media_error("screen changed during source startup"));
            }
            Ok(Box::new(NativeSource { process, index: 0 }) as Box<dyn DesktopSource>)
        })
    }
}
pub struct NativeViewer {
    process: MediaChild,
    config: DesktopVideoConfig,
    next_frame: u64,
    renderer: crate::renderer::Renderer,
}
impl NativeViewer {
    pub async fn open(
        path: &PathBuf,
        prefix: &[String],
        config: &DesktopVideoConfig,
        verify: bool,
    ) -> std::result::Result<Self, AnyError> {
        Self::open_inner(path, prefix, config, verify, None, false, false).await
    }
    /// Explicit diagnostic/fallback selection; normal callers use automatic
    /// hardware selection and retain the direct GPU path when it is available.
    pub async fn open_software(
        path: &PathBuf,
        prefix: &[String],
        config: &DesktopVideoConfig,
        verify: bool,
    ) -> std::result::Result<Self, AnyError> {
        Self::open_inner(path, prefix, config, verify, None, false, true).await
    }
    /// Parent HWND is obtained by the native Desktop host, never from a remote
    /// message or webview argument. The child leaves 72 logical px for controls.
    pub async fn open_in_window(
        path: &PathBuf,
        prefix: &[String],
        config: &DesktopVideoConfig,
        parent: usize,
    ) -> std::result::Result<Self, AnyError> {
        Self::open_inner(path, prefix, config, false, Some(parent), false, false).await
    }
    pub async fn open_interactive(
        path: &PathBuf,
        prefix: &[String],
        config: &DesktopVideoConfig,
        parent: usize,
    ) -> std::result::Result<(Self, crate::NativeViewerInput), AnyError> {
        let mut viewer =
            Self::open_inner(path, prefix, config, false, Some(parent), true, false).await?;
        let events = crate::NativeViewerInput::new(
            viewer.process.input.clone(),
            viewer.process.events.take().unwrap(),
        );
        Ok((viewer, events))
    }
    async fn open_inner(
        path: &PathBuf,
        prefix: &[String],
        config: &DesktopVideoConfig,
        verify: bool,
        parent: Option<usize>,
        interactive: bool,
        software: bool,
    ) -> std::result::Result<Self, AnyError> {
        config.validate()?;
        let mut arguments = vec![
            "render".into(),
            config.screen.pixel_width.to_string(),
            config.screen.pixel_height.to_string(),
            config.fps.to_string(),
            if interactive {
                "stream-interactive"
            } else if verify {
                "stream-verify"
            } else {
                "stream"
            }
            .into(),
        ];
        if software {
            arguments[4].push_str("-software");
        }
        arguments.push(parent.unwrap_or(0).to_string());
        arguments.push(
            match config.screen.rotation {
                DesktopRotation::Identity => "0",
                DesktopRotation::Clockwise90 => "90",
                DesktopRotation::Clockwise180 => "180",
                DesktopRotation::Clockwise270 => "270",
            }
            .into(),
        );
        let mut process = MediaChild::spawn(path, prefix, &arguments, None)?;
        process.ready().await?;
        if process.output.as_mut().unwrap().read_u32_le().await? != 2 {
            return Err("native renderer protocol mismatch".into());
        }
        let renderer = crate::renderer::Renderer::start(process.output.take().unwrap());
        Ok(Self {
            process,
            config: config.clone(),
            next_frame: 1,
            renderer,
        })
    }
    pub fn presentations(&self) -> crate::renderer::NativePresentations {
        self.renderer.presentations()
    }
    /// Submit compressed data with bounded decoder backpressure. Rendering is
    /// reported independently by presentations(); consumption is never a video ACK.
    pub async fn submit(&mut self, frame: &VideoFrame) -> std::result::Result<(), AnyError> {
        frame.header.validate(&self.config, self.next_frame)?;
        if frame.bytes.len() != frame.header.payload_bytes as usize {
            return Err("native frame payload length mismatch".into());
        }
        self.renderer.enqueue(frame.header.clone()).await?;
        {
            let mut input = self.process.input.lock().await;
            input.write_u32_le(frame.bytes.len().try_into()?).await?;
            input.write_u64_le(frame.header.frame_id - 1).await?;
            input.write_all(&frame.bytes).await?;
        }
        self.renderer.consumed(frame.header.frame_id).await?;
        self.next_frame = self
            .next_frame
            .checked_add(1)
            .ok_or("native frame sequence exhausted")?;
        Ok(())
    }
    pub async fn finish(mut self) -> std::result::Result<serde_json::Value, AnyError> {
        self.process.input.lock().await.write_u32_le(0).await?;
        let summary =
            tokio::time::timeout(std::time::Duration::from_secs(5), self.renderer.summary())
                .await??;
        if !self.process.child.as_mut().unwrap().wait().await?.success() {
            return Err("native viewer failed on shutdown".into());
        }
        Ok(summary)
    }
}
