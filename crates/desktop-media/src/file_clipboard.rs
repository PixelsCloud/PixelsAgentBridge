//! Dedicated user-token file clipboard worker. Files are opened only by opaque
//! offer/index, and native OLE callbacks never perform network or path lookups.
use pab_protocol::*;
use std::{io, path::PathBuf, process::Stdio};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout},
    sync::Mutex,
};
pub struct NativeFileClipboard {
    executable: PathBuf,
    prefix: Vec<String>,
    worker: Mutex<Option<Worker>>,
}
struct Worker {
    _child: Child,
    input: ChildStdin,
    output: ChildStdout,
}
impl NativeFileClipboard {
    pub fn new(executable: PathBuf, prefix: Vec<String>) -> Self {
        Self {
            executable,
            prefix,
            worker: Mutex::new(None),
        }
    }
    pub async fn exchange(
        &self,
        operation: FileClipboardOperation,
    ) -> io::Result<FileClipboardSnapshot> {
        operation.validate().map_err(io::Error::other)?;
        let mut slot = self.worker.lock().await;
        let mut worker = match slot.take() {
            Some(worker) => worker,
            None => {
                let mut command = tokio::process::Command::new(&self.executable);
                command
                    .args(&self.prefix)
                    .arg("file-clipboard")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                #[cfg(windows)]
                command.creation_flags(0x08000000);
                let mut child = command.spawn()?;
                Worker {
                    input: child
                        .stdin
                        .take()
                        .ok_or_else(|| io::Error::other("file_clipboard_unavailable"))?,
                    output: child
                        .stdout
                        .take()
                        .ok_or_else(|| io::Error::other("file_clipboard_unavailable"))?,
                    _child: child,
                }
            }
        };
        let work = async {
            let bytes = serde_json::to_vec(&operation)?;
            if bytes.len() > FILE_CLIPBOARD_HEADER {
                return Err(io::Error::other("file_clipboard_too_many_files"));
            }
            worker.input.write_u32_le(bytes.len() as u32).await?;
            worker.input.write_all(&bytes).await?;
            worker.input.write_all(operation.payload()).await?;
            worker.input.flush().await?;
            let length = worker.output.read_u32_le().await? as usize;
            if length > FILE_CLIPBOARD_HEADER {
                return Err(io::Error::other("file_clipboard_invalid_reply"));
            }
            let mut bytes = vec![0; length];
            worker.output.read_exact(&mut bytes).await?;
            let mut response: Result<FileClipboardSnapshot, String> =
                serde_json::from_slice(&bytes)?;
            if let Ok(snapshot) = &mut response {
                snapshot.validate().map_err(io::Error::other)?;
                snapshot.data.resize(snapshot.bytes as usize, 0);
                worker.output.read_exact(&mut snapshot.data).await?;
                verify_payload(snapshot)?;
            }
            Ok::<_, io::Error>(response)
        };
        // Cancellation or a partial frame drops this child instead of reusing its pipe.
        let value = tokio::time::timeout(std::time::Duration::from_secs(12), work)
            .await
            .map_err(|_| io::Error::other("file_clipboard_timeout"))??;
        *slot = Some(worker);
        value.map_err(io::Error::other)
    }
}
pub fn verify_payload(snapshot: &FileClipboardSnapshot) -> io::Result<()> {
    use sha2::{Digest, Sha256};
    snapshot.validate().map_err(io::Error::other)?;
    if snapshot.data.len() != snapshot.bytes as usize
        || snapshot.bytes > 0
            && snapshot.sha256.as_deref()
                != Some(format!("{:x}", Sha256::digest(&snapshot.data)).as_str())
    {
        return Err(io::Error::other("file_clipboard_invalid_digest"));
    }
    Ok(())
}

#[cfg(windows)]
#[path = "file_clipboard_windows.rs"]
mod windows;
#[cfg(windows)]
pub fn worker_main() -> i32 {
    use std::io::{Read, Write};
    match crate::clipboard::windows_user::run_if_system() {
        Ok(Some(code)) => return code,
        Ok(None) => {}
        Err(_) => return 1,
    }
    let Ok(mut state) = windows::State::new() else {
        return 1;
    };
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let mut size = [0; 4];
        if input.read_exact(&mut size).is_err() {
            return 0;
        }
        let size = u32::from_le_bytes(size) as usize;
        if size > FILE_CLIPBOARD_HEADER {
            return 1;
        }
        let mut bytes = vec![0; size];
        if input.read_exact(&mut bytes).is_err() {
            return 1;
        }
        let Ok(mut operation) = serde_json::from_slice::<FileClipboardOperation>(&bytes) else {
            return 1;
        };
        if operation.validate().is_err() {
            return 1;
        }
        if let FileClipboardOperation::Deliver { bytes, data, .. } = &mut operation {
            data.resize(*bytes as usize, 0);
            if input.read_exact(data).is_err() {
                return 1;
            }
        }
        let result = state.exchange(operation).map_err(|error| error.to_string());
        let payload = result
            .as_ref()
            .ok()
            .map(|s| s.data.as_slice())
            .unwrap_or_default();
        let Ok(header) = serde_json::to_vec(&result) else {
            return 1;
        };
        if header.len() > FILE_CLIPBOARD_HEADER {
            return 1;
        }
        if output
            .write_all(&(header.len() as u32).to_le_bytes())
            .and_then(|_| output.write_all(&header))
            .and_then(|_| output.write_all(payload))
            .and_then(|_| output.flush())
            .is_err()
        {
            return 0;
        }
    }
}
#[cfg(not(windows))]
pub fn worker_main() -> i32 {
    1
}
