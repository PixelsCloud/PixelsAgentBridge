//! A separate pipe worker owns all injected key/button state. Closing its pipe
//! is graceful, including cancellation half-way through an event or its ACK.
#![forbid(unsafe_code)]
use crate::process::media_error;
use pab_protocol::{DesktopMouseButton, DesktopVideoConfig, RemoteDesktopInput};
use pab_remote_desktop::{DesktopInputSink, MediaFuture};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{ChildStdin, ChildStdout, Command},
    sync::{OwnedMutexGuard, oneshot},
};

fn command(executable: &PathBuf, prefix: &[String]) -> Command {
    let mut command = Command::new(executable);
    command
        .args(prefix)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    command
}

pub(crate) async fn recover(executable: &PathBuf, prefix: &[String]) -> std::io::Result<()> {
    let mut child = command(executable, prefix)
        .arg("recover-input")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let status = tokio::time::timeout(Duration::from_secs(3), child.wait()).await??;
    if !status.success() {
        return Err(std::io::Error::other("input release recovery failed"));
    }
    Ok(())
}

pub(crate) struct NativeInput {
    input: Option<ChildStdin>,
    output: ChildStdout,
    stop: Option<oneshot::Sender<()>>,
}
impl NativeInput {
    pub(crate) async fn open(
        executable: PathBuf,
        prefix: Vec<String>,
        config: DesktopVideoConfig,
        permit: OwnedMutexGuard<()>,
    ) -> std::io::Result<Self> {
        // Deliberately no kill_on_drop: closing stdin lets native release keys.
        let mut child = command(&executable, &prefix)
            .args(["input", &config.screen.id])
            .spawn()?;
        let (stop, mut stopped) = oneshot::channel();
        let mut sink = Self {
            input: child.stdin.take(),
            output: child.stdout.take().unwrap(),
            stop: Some(stop),
        };
        tokio::spawn(async move {
            // Watch from startup, not only Drop: an idle worker can crash while
            // Ctrl is held and no further event may ever arrive to detect it.
            tokio::select! {
                _ = child.wait() => {},
                _ = &mut stopped => {
                    if tokio::time::timeout(Duration::from_secs(3), child.wait()).await.is_err() {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                    }
                }
            }
            for _ in 0..3 {
                if recover(&executable, &prefix).await.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            // New input cannot overlap the prior worker or its release pass.
            drop(permit);
        });
        let input = sink.input.as_mut().unwrap();
        input.write_i32_le(config.screen.x).await?;
        input.write_i32_le(config.screen.y).await?;
        input.write_u32_le(config.screen.desktop_width).await?;
        input.write_u32_le(config.screen.desktop_height).await?;
        if sink.output.read_u32_le().await? != super::process::MAGIC {
            return Err(std::io::Error::other("native input IPC version mismatch"));
        }
        Ok(sink)
    }
}
impl DesktopInputSink for NativeInput {
    fn apply(&mut self, event: RemoteDesktopInput) -> MediaFuture<'_, ()> {
        Box::pin(async move {
            event.validate().map_err(media_error)?;
            let packet = encode(&event);
            self.input
                .as_mut()
                .unwrap()
                .write_all(&packet)
                .await
                .map_err(media_error)?;
            if self.output.read_u8().await.map_err(media_error)? != 1 {
                return Err(media_error("invalid native input acknowledgement"));
            }
            Ok(())
        })
    }
}
impl Drop for NativeInput {
    fn drop(&mut self) {
        // Close the pipe first. A pending read, including an incomplete packet,
        // observes EOF and releases without waiting for another input event.
        drop(self.input.take());
        drop(self.stop.take());
        // On runtime shutdown the native process still owns stdin EOF and the
        // durable release state. A new helper recovers before registration.
    }
}

fn encode(event: &RemoteDesktopInput) -> [u8; 8] {
    let (kind, x, y, value, down) = match *event {
        RemoteDesktopInput::Move { x, y } => (1, x, y, 0, false),
        RemoteDesktopInput::Button { x, y, button, down } => (
            2,
            x,
            y,
            match button {
                DesktopMouseButton::Left => 0,
                DesktopMouseButton::Right => 1,
                DesktopMouseButton::Middle => 2,
            },
            down,
        ),
        RemoteDesktopInput::Wheel { delta } => (3, 0, 0, delta as u16, false),
        RemoteDesktopInput::Key { virtual_key, down } => (4, 0, 0, virtual_key, down),
        RemoteDesktopInput::ReleaseAll => (5, 0, 0, 0, false),
    };
    let x = x.to_le_bytes();
    let y = y.to_le_bytes();
    let value = value.to_le_bytes();
    [
        kind,
        x[0],
        x[1],
        y[0],
        y[1],
        value[0],
        value[1],
        u8::from(down),
    ]
}
