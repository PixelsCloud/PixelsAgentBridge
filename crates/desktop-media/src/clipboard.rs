//! Persistent text-only OS clipboard owner. A private child isolates native
//! clipboard stalls from capture/input and keeps Linux selection data alive.
use pab_protocol::*;
use std::{io, path::PathBuf, process::Stdio};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, process::{Child, ChildStdin, ChildStdout}, sync::Mutex};
#[cfg(target_os = "linux")]
#[path = "linux_clipboard_environment.rs"]
mod linux_environment;

pub struct NativeClipboard {
    executable: PathBuf,
    prefix: Vec<String>,
    worker: Mutex<Option<Worker>>,
}
struct Worker { _child: Child, input: ChildStdin, output: ChildStdout }
impl NativeClipboard {
    pub fn new(executable: PathBuf, prefix: Vec<String>) -> Self { Self { executable, prefix, worker: Mutex::new(None) } }
    pub async fn exchange(&self, operation: DesktopClipboardOperation) -> io::Result<DesktopClipboardSnapshot> {
        operation.validate().map_err(io::Error::other)?;
        let mut slot = self.worker.lock().await;
        // Cancellation drops the partially consumed pipe instead of reusing it.
        let mut worker = match slot.take() {
            Some(worker) => worker,
            None => {
                let mut command = tokio::process::Command::new(&self.executable);
                command.args(&self.prefix).arg("clipboard").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
                #[cfg(windows)] command.creation_flags(0x08000000);
                let mut child = command.spawn()?;
                Worker { input: child.stdin.take().ok_or_else(|| io::Error::other("clipboard stdin unavailable"))?,
                    output: child.stdout.take().ok_or_else(|| io::Error::other("clipboard stdout unavailable"))?, _child: child }
            }
        };
        let request = serde_json::to_vec(&operation)?;
        let work = async {
            worker.input.write_u32_le(request.len() as u32).await?;
            worker.input.write_all(&request).await?;
            worker.input.flush().await?;
            let size = worker.output.read_u32_le().await? as usize;
            if size > DESKTOP_CLIPBOARD_JSON_BYTES { return Err(io::Error::other("clipboard reply too large")); }
            let mut bytes = vec![0; size]; worker.output.read_exact(&mut bytes).await?;
            let response: Option<DesktopClipboardSnapshot> = serde_json::from_slice(&bytes)?;
            if let Some(snapshot) = &response { snapshot.validate().map_err(io::Error::other)?; }
            Ok::<_, io::Error>(response)
        };
        let response = tokio::time::timeout(std::time::Duration::from_secs(2), work).await.map_err(|_| io::Error::other("clipboard timed out"))??;
        *slot = Some(worker);
        response.ok_or_else(|| io::Error::other("system text clipboard is temporarily unavailable"))
    }
}

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
pub fn worker_main() -> i32 {
    use std::io::{Read, Write};
    // A supervised GNOME helper can have only the verified session bus: the
    // compositor publishes Xwayland/Wayland addresses after its own exec.
    // Resolve them only in this private, single-threaded clipboard child.
    #[cfg(target_os = "linux")]
    linux_environment::initialize();
    // Retry native occupancy briefly, as RustDesk does; never log clipboard data.
    fn read(board: &mut arboard::Clipboard) -> Result<Option<String>, ()> {
        for attempt in 0..3 {
            match board.get_text() {
                Ok(text) => return validate_clipboard_text(&text).map(|_| Some(text)).map_err(|_| ()),
                Err(arboard::Error::ContentNotAvailable) => return Ok(None),
                Err(arboard::Error::ClipboardOccupied) if attempt < 2 => std::thread::sleep(std::time::Duration::from_millis(33)),
                Err(_) => return Err(()),
            }
        }
        Err(())
    }
    let mut board = None;
    let mut last: Option<Option<String>> = None;
    let mut revision = 1u64;
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let mut size = [0; 4];
        if input.read_exact(&mut size).is_err() { return 0; }
        let size = u32::from_le_bytes(size) as usize;
        if size > DESKTOP_CLIPBOARD_JSON_BYTES { return 1; }
        let mut bytes = vec![0; size]; if input.read_exact(&mut bytes).is_err() { return 1; }
        let Ok(operation) = serde_json::from_slice::<DesktopClipboardOperation>(&bytes) else { return 1; };
        if operation.validate().is_err() { return 1; }
        let result = (|| {
            if board.is_none() { board = arboard::Clipboard::new().ok(); }
            let board = board.as_mut().ok_or(())?;
            match operation {
                DesktopClipboardOperation::Read { after } => {
                    let text = read(board)?;
                    if last.as_ref() != Some(&text) { revision = revision.checked_add(1).ok_or(())?; last = Some(text.clone()); }
                    let changed = after != Some(revision);
                    Ok(DesktopClipboardSnapshot { revision, changed, text: if changed { text } else { None } })
                }
                DesktopClipboardOperation::Write { text } => {
                    // Do not claim the clipboard again for an echoed update.
                    if read(board).ok().flatten().as_ref() != Some(&text) {
                        let mut written = false;
                        for attempt in 0..3 {
                            match board.set_text(text.clone()) {
                                Ok(()) => { written = true; break; }
                                Err(arboard::Error::ClipboardOccupied) if attempt < 2 => std::thread::sleep(std::time::Duration::from_millis(33)),
                                Err(_) => return Err(()),
                            }
                        }
                        if !written { return Err(()); }
                    }
                    if last.as_ref() != Some(&Some(text.clone())) { revision = revision.checked_add(1).ok_or(())?; }
                    last = Some(Some(text));
                    Ok(DesktopClipboardSnapshot { revision, changed: false, text: None })
                }
            }
        })();
        let Ok(bytes) = serde_json::to_vec(&result.ok()) else { return 1; };
        if output.write_all(&(bytes.len() as u32).to_le_bytes()).and_then(|_| output.write_all(&bytes)).and_then(|_| output.flush()).is_err() { return 1; }
    }
}
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub fn worker_main() -> i32 { 1 }
