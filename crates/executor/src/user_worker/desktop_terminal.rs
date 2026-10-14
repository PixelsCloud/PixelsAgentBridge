//! Separate authenticated IPC output and control channels; output backpressure
//! cannot prevent Ctrl+C/resize, and parent Job termination never waits on input.
use super::*;
use pab_interactive_terminal::{InteractivePty, PtyOutput};
use pab_protocol::{DESKTOP_TERMINAL_CHUNK, DesktopTerminalControl};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub(crate) struct Started {
    pub control: WorkerControl,
    pub output: channel::WorkerStream,
    pub child: pab_os_control::execution::UserProcess,
    pub shell: String,
    pub arguments: Vec<String>,
    pub cwd: String,
}
pub(crate) struct WorkerControl(Peer);
impl WorkerControl {
    pub(crate) async fn apply(&mut self, request: DesktopTerminalControl) -> io::Result<()> {
        self.0
            .control(Control::DesktopTerminalControl { request })
            .await?;
        match decode(self.0.receive().await?)? {
            Control::DesktopTerminalApplied { error: None } => Ok(()),
            Control::DesktopTerminalApplied { error: Some(e) } => Err(io::Error::other(e)),
            _ => Err(io::Error::other(
                "unexpected human terminal worker acknowledgement",
            )),
        }
    }
}
pub(crate) enum WorkerOutput {
    Bytes(Vec<u8>),
    End(Vec<u8>),
}
pub(crate) async fn read_output(output: &mut (impl AsyncRead + Unpin)) -> io::Result<WorkerOutput> {
    let frame = read_frame(output).await?;
    match frame.tag {
        5 if !frame.bytes.is_empty() && frame.bytes.len() <= DESKTOP_TERMINAL_CHUNK => {
            Ok(WorkerOutput::Bytes(frame.bytes))
        }
        6 if frame.bytes.len() <= 4096 => Ok(WorkerOutput::End(frame.bytes)),
        _ => Err(io::Error::other("invalid human terminal worker output")),
    }
}
pub(crate) async fn start(
    executable: &Path,
    prepared: PreparedUser,
    cols: u16,
    rows: u16,
) -> io::Result<Started> {
    let mut output = channel::WorkerListener::bind(prepared.identity())?;
    let (mut peer, child) = super::launch(executable, prepared).await?;
    peer.control(Control::DesktopTerminalOpen {
        cols,
        rows,
        output_address: output.address().into(),
    })
    .await?;
    let socket = output.accept(child.id(), HANDSHAKE).await?;
    let reply = tokio::time::timeout(HANDSHAKE, peer.receive())
        .await
        .map_err(io::Error::other)??;
    match decode(reply)? {
        Control::DesktopTerminalOpened {
            shell,
            arguments,
            cwd,
        } => Ok(Started {
            control: WorkerControl(peer),
            output: socket,
            child,
            shell,
            arguments,
            cwd,
        }),
        _ => Err(io::Error::other("unexpected Desktop terminal worker reply")),
    }
}
fn shell() -> io::Result<(String, Vec<String>)> {
    if cfg!(windows) {
        return Ok((
            "powershell.exe".into(),
            vec![
                "-NoLogo".into(), "-NoExit".into(), "-Command".into(),
                // ConPTY carries UTF-8. Windows PowerShell otherwise keeps the
                // machine's legacy console codepage and corrupts characters
                // that it cannot represent. Profiles still load normally.
                "[Console]::InputEncoding=[System.Text.UTF8Encoding]::new(); [Console]::OutputEncoding=[System.Text.UTF8Encoding]::new(); $OutputEncoding=[System.Text.UTF8Encoding]::new()".into(),
            ],
        ));
    }
    if cfg!(target_os = "macos") {
        return Ok(("/bin/zsh".into(), vec!["-l".into(), "-i".into()]));
    }
    let shell = std::env::var("SHELL").map_err(io::Error::other)?;
    if !Path::new(&shell).is_absolute() || !Path::new(&shell).is_file() {
        return Err(io::Error::other("account login Shell unavailable"));
    }
    Ok((shell, vec!["-l".into(), "-i".into()]))
}

pub(super) async fn serve(
    mut control: Peer,
    cols: u16,
    rows: u16,
    address: &str,
    parent: u32,
) -> io::Result<()> {
    let output = channel::connect(address, parent, HANDSHAKE).await?;
    let (shell, arguments) = shell()?;
    let cwd = current_identity()?.home;
    let start_shell = shell.clone();
    let start_args = arguments.clone();
    let start_cwd = cwd.clone();
    let (pty, mut incoming) = tokio::task::spawn_blocking(move || {
        InteractivePty::start(&start_shell, &start_args, &start_cwd, cols, rows)
    })
    .await
    .map_err(io::Error::other)??;
    control
        .control(Control::DesktopTerminalOpened {
            shell,
            arguments,
            cwd: cwd.to_string_lossy().into(),
        })
        .await?;
    let output_pty = Arc::clone(&pty);
    let (mut close_reader, output_writer) = tokio::io::split(output);
    let close_pty = Arc::clone(&pty);
    let _close_listener = AbortOnDrop(tokio::spawn(async move {
        // Authenticated, separately reachable shutdown direction. Closing the
        // owned PTY signals attached programs without waiting on the input lock.
        if matches!(close_reader.read_u8().await, Ok(1)) {
            let _ = close_pty.drain_after_exit();
        }
    }));
    let mut pump = AbortOnDrop(tokio::spawn(async move {
        let mut output = output_writer;
        let mut pending = Vec::with_capacity(DESKTOP_TERMINAL_CHUNK);
        let mut exiting = false;
        let mut check = tokio::time::interval(Duration::from_millis(50));
        let mut flush = tokio::time::interval(Duration::from_millis(10));
        let mut end = None;
        let mut digest = Sha256::new();
        let mut output_bytes = 0_u64;
        loop {
            tokio::select! {
                item = incoming.recv() => match item {
                    Some(PtyOutput::Bytes(bytes)) => {
                        digest.update(&bytes);
                        output_bytes = output_bytes.checked_add(bytes.len() as u64).ok_or_else(|| io::Error::other("PTY output offset overflow"))?;
                        let mut rest = bytes.as_slice();
                        while !rest.is_empty() {
                            let size = rest.len().min(DESKTOP_TERMINAL_CHUNK - pending.len());
                            pending.extend_from_slice(&rest[..size]); rest = &rest[size..];
                            if pending.len() >= DESKTOP_TERMINAL_CHUNK { write_frame(&mut output, 5, &pending).await?; pending.clear(); }
                        }
                    }
                    Some(PtyOutput::End) | None => { end = Some(None); }
                    Some(PtyOutput::Error(e)) => { end = Some(Some(e)); }
                },
                _ = flush.tick(), if !pending.is_empty() => { write_frame(&mut output, 5, &pending).await?; pending.clear(); },
                _ = check.tick(), if !exiting => {
                    if output_pty.exit_code()?.is_some() { exiting = true; output_pty.drain_after_exit()?; }
                }
            }
            if let Some(error) = end.take() {
                if !pending.is_empty() {
                    write_frame(&mut output, 5, &pending).await?;
                }
                let mut exit = output_pty.exit_code()?;
                // After real EOF (or Unix PTY EIO), allow the owning process to
                // publish its exit status. This never guesses output completion.
                let until = tokio::time::Instant::now() + Duration::from_secs(1);
                while exit.is_none() && tokio::time::Instant::now() < until {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    exit = output_pty.exit_code()?;
                }
                let value = serde_json::json!({"exit_code": exit, "reason": error.unwrap_or_else(|| if exit.is_some() {"Shell exited".into()} else {"PTY output ended unexpectedly".into()}), "output_bytes": output_bytes, "sha256": hex::encode(digest.finalize())});
                write_frame(&mut output, 6, &serde_json::to_vec(&value)?).await?;
                return Ok::<(), io::Error>(());
            }
        }
    }));
    let mut sequence = 1_u64;
    loop {
        let value = tokio::select! {
            item = control.receive() => item?,
            result = &mut pump.0 => { let _ = pty.kill(); return result.map_err(io::Error::other)?; }
        };
        let Control::DesktopTerminalControl { request } = decode(value)? else {
            return Err(io::Error::other("unexpected human terminal control"));
        };
        request.validate().map_err(io::Error::other)?;
        let copy = Arc::clone(&pty);
        let result = match request {
            DesktopTerminalControl::Input {
                sequence: actual,
                bytes,
            } if actual == sequence => {
                sequence = sequence
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("input sequence overflow"))?;
                tokio::task::spawn_blocking(move || copy.input(&bytes))
                    .await
                    .map_err(io::Error::other)?
            }
            DesktopTerminalControl::Resize { cols, rows } => {
                tokio::task::spawn_blocking(move || copy.resize(cols, rows))
                    .await
                    .map_err(io::Error::other)?
            }
            _ => {
                return Err(io::Error::other(
                    "out-of-order input; input is never replayed",
                ));
            }
        };
        control
            .control(Control::DesktopTerminalApplied {
                error: result.err().map(|e| e.to_string()),
            })
            .await?;
    }
}
