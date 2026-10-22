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
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
#[path = "cursor_source.rs"]
mod cursor;
#[path = "diagnostics.rs"]
mod diagnostics;
#[cfg(target_os = "linux")]
#[path = "linux_cursor_channel.rs"]
mod linux_cursor_channel;
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
    diagnostics: Option<diagnostics::Diagnostics>,
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
            && args.get(4).is_some_and(|s| {
                matches!(
                    s.as_str(),
                    "interactive"
                        | "stream-interactive"
                        | "interactive-software"
                        | "stream-interactive-software"
                )
            });
        command
            .args(prefix)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000); // No console; the render window is created explicitly.
        let mut child = command.spawn()?;
        let stderr = child.stderr.take().unwrap();
        let (events, diagnostics) = if interactive {
            (Some(stderr), None)
        } else {
            (None, Some(diagnostics::Diagnostics::start(stderr)))
        };
        Ok(Self {
            input: Arc::new(Mutex::new(child.stdin.take().unwrap())),
            output: child.stdout.take(),
            events,
            diagnostics,
            child: Some(child),
            permit,
        })
    }
    async fn ready(&mut self) -> std::io::Result<()> {
        let magic = match self.output.as_mut().unwrap().read_u32_le().await {
            Ok(value) => value,
            Err(error) => return Err(self.failure(error).await),
        };
        if magic != MAGIC {
            return Err(std::io::Error::other("media IPC version mismatch"));
        }
        Ok(())
    }
    async fn failure(&mut self, error: impl std::fmt::Display) -> std::io::Error {
        let message = match self.diagnostics.as_mut() {
            Some(diagnostics) => diagnostics.context(error).await,
            None => error.to_string(),
        };
        std::io::Error::other(message)
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
    clipboard: crate::clipboard::NativeClipboard,
    executable: PathBuf,
    prefix: Vec<String>,
    gate: Arc<Mutex<()>>,
    input_gate: Arc<Mutex<()>>,
    input_enabled: bool,
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    cursor_gate: Arc<Mutex<()>>,
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    capture: std::sync::Mutex<Option<std::sync::Weak<cursor::CaptureLease>>>,
}
impl NativeBackend {
    pub fn new(executable: PathBuf, prefix: Vec<String>) -> Self {
        Self {
            clipboard: crate::clipboard::NativeClipboard::new(executable.clone(), prefix.clone()),
            executable,
            prefix,
            gate: Arc::new(Mutex::new(())),
            input_gate: Arc::new(Mutex::new(())),
            input_enabled: false,
            #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
            cursor_gate: Arc::new(Mutex::new(())),
            #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
            capture: std::sync::Mutex::new(None),
        }
    }
    pub async fn idle(&self) {
        drop(self.gate.lock().await);
        drop(self.input_gate.lock().await);
        #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
        drop(self.cursor_gate.lock().await);
    }
    /// Only the dedicated interactive media helper enables OS input. Capture
    /// experiments and all other NativeBackend callers remain view-only.
    pub fn with_input(mut self) -> Self {
        self.input_enabled = cfg!(any(windows, target_os = "linux"));
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
    encoding_budget: Option<u32>,
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    capture: Arc<cursor::CaptureLease>,
}
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
impl Drop for NativeSource {
    fn drop(&mut self) {
        self.capture.ended.send_replace(true);
    }
}
impl DesktopSource for NativeSource {
    fn encoding_budget(&self) -> Option<u32> { self.encoding_budget }
    fn set_encoding_budget(&mut self, bits_per_second: u32) -> MediaFuture<'_, ()> {
        Box::pin(async move {
            if bits_per_second != 0 && !(150_000..=16_000_000).contains(&bits_per_second) {
                return Err(media_error("encoder budget outside bounds"));
            }
            {
                let mut input = self.process.input.lock().await;
                input.write_u8(2).await.map_err(media_error)?;
                input.write_u32_le(bits_per_second).await.map_err(media_error)?;
            }
            self.encoding_budget = match self.process.output.as_mut().unwrap().read_u32_le().await.map_err(media_error)? {
                MAGIC => Some(bits_per_second),
                value if value == MAGIC + 1 => None,
                _ => return Err(media_error("encoder budget acknowledgment mismatch")),
            };
            Ok(())
        })
    }
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
            match operation.await {
                Ok(frame) => Ok(frame),
                Err(error) => Err(media_error(self.process.failure(error).await)),
            }
        })
    }
}
impl DesktopBackend for NativeBackend {
    fn supports_clipboard(&self) -> bool { cfg!(any(windows, target_os = "macos", target_os = "linux")) }
    fn clipboard(&self, config: DesktopVideoConfig, operation: DesktopClipboardOperation) -> MediaFuture<'_, DesktopClipboardSnapshot> {
        Box::pin(async move {
            config.validate().map_err(media_error)?;
            self.clipboard.exchange(operation).await.map_err(|_| media_error("system text clipboard is temporarily unavailable"))
        })
    }
    fn supports_cursor(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            linux_cursor_channel::supported()
        }
        #[cfg(not(target_os = "linux"))]
        {
            cfg!(any(windows, target_os = "macos"))
        }
    }
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    fn cursor(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopCursorSource>> {
        Box::pin(cursor::open(self, config))
    }
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
                return Err(media_error(
                    process.failure("screen inventory failed").await,
                ));
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
            #[cfg(windows)]
            let capture_token = RequestId::new().to_string();
            #[cfg(target_os = "linux")]
            let channel = if self.supports_cursor() && config.screen.id.starts_with("gnome:") {
                Some(linux_cursor_channel::Channel::new().map_err(media_error)?)
            } else {
                None
            };
            let args = vec![
                "capture".into(),
                config.screen.id.clone(),
                config.fps.to_string(),
                if cfg!(target_os = "macos") {
                    "desktop-separate-cursor".into()
                } else {
                    "desktop".into()
                },
            ];
            #[cfg(windows)]
            let args = {
                let mut args = args;
                args.push(capture_token.clone());
                args
            };
            #[cfg(target_os = "linux")]
            let args = {
                let mut args = args;
                if let Some(channel) = &channel {
                    args[3] = "desktop-separate-cursor".into();
                    args.push(
                        channel
                            .path
                            .to_str()
                            .ok_or_else(|| media_error("cursor channel path is not UTF-8"))?
                            .into(),
                    );
                } else if linux_cursor_channel::x11_selected()
                    && config.screen.id.starts_with("x11:")
                {
                    args[3] = "desktop-separate-cursor".into();
                }
                args
            };
            let mut process =
                MediaChild::spawn(&self.executable, &self.prefix, &args, Some(permit))
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
            #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
            let capture = {
                let lease = Arc::new(cursor::CaptureLease::new(
                    config,
                    #[cfg(windows)]
                    capture_token,
                    #[cfg(target_os = "linux")]
                    channel,
                ));
                *self
                    .capture
                    .lock()
                    .map_err(|_| media_error("capture state unavailable"))? =
                    Some(Arc::downgrade(&lease));
                lease
            };
            Ok(Box::new(NativeSource {
                process,
                index: 0,
                encoding_budget: Some(0),
                #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
                capture,
            }) as Box<dyn DesktopSource>)
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
        Self::open_interactive_with_software(path, prefix, config, parent, false).await
    }
    /// A fresh viewer after a renderer failure can explicitly select software.
    /// This does not reuse the failed generation's pending frames or input.
    pub async fn open_interactive_with_software(
        path: &PathBuf,
        prefix: &[String],
        config: &DesktopVideoConfig,
        parent: usize,
        software: bool,
    ) -> std::result::Result<(Self, crate::NativeViewerInput), AnyError> {
        let mut viewer =
            Self::open_inner(path, prefix, config, false, Some(parent), true, software).await?;
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
    #[cfg(windows)]
    pub fn cursor_renderer(&self) -> std::result::Result<crate::NativeCursorRenderer, AnyError> {
        Ok(crate::NativeCursorRenderer::new(
            &self.process.input,
            self.config.clone(),
            self.renderer.cursor()?,
        ))
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
