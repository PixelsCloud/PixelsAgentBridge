//! Persistent text/image OS clipboard owner. A private child isolates native
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
            if let Some(image) = operation.image() { worker.input.write_all(&image.data).await?; }
            worker.input.flush().await?;
            let size = worker.output.read_u32_le().await? as usize;
            if size > DESKTOP_CLIPBOARD_JSON_BYTES { return Err(io::Error::other("clipboard reply too large")); }
            let mut bytes = vec![0; size]; worker.output.read_exact(&mut bytes).await?;
            let mut response: Result<DesktopClipboardSnapshot, String> = serde_json::from_slice(&bytes)?;
            if let Ok(snapshot) = &mut response {
                if let Some(image) = &mut snapshot.image {
                    image.validate_header().map_err(io::Error::other)?;
                    image.data.resize(image.bytes as usize, 0);
                    worker.output.read_exact(&mut image.data).await?;
                }
                snapshot.validate().map_err(io::Error::other)?;
            }
            Ok::<_, io::Error>(response)
        };
        let response = tokio::time::timeout(std::time::Duration::from_secs(DESKTOP_CLIPBOARD_TIMEOUT_SECS), work).await.map_err(|_| io::Error::other("clipboard timed out"))??;
        *slot = Some(worker);
        response.map_err(io::Error::other)
    }
}


#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod native {
    use super::*;
    use crate::clipboard_image::error;
    #[cfg(not(windows))] use crate::clipboard_image::{encode, decode};
    type Content = (Option<String>, Option<DesktopClipboardImage>);
    pub(super) struct State {
        #[cfg(windows)] board: crate::clipboard_windows::Board,
        #[cfg(not(windows))] board: arboard::Clipboard,
        content: Option<Content>, sequence: Option<u64>, revision: u64,
        #[cfg(not(windows))] raw: Option<arboard::ImageData<'static>>,
    }
    impl State {
        pub(super) fn new() -> io::Result<Self> {
            Ok(Self {
                #[cfg(windows)] board: crate::clipboard_windows::Board::new()?,
                #[cfg(not(windows))] board: arboard::Clipboard::new().map_err(|_|error("clipboard_unavailable"))?,
                content:None,sequence:None,
                // Worker restarts must not reuse the previous owner's revisions.
                revision: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64 | 1,
                #[cfg(not(windows))] raw:None,
            })
        }
        fn refresh(&mut self) -> io::Result<()> {
            #[cfg(windows)]
            let (content, sequence) = {
                let before = crate::clipboard_windows::sequence();
                if self.content.is_some() && self.sequence == before { return Ok(()); }
                let content = crate::clipboard_windows::read()?;
                let after = crate::clipboard_windows::sequence();
                if before != after { return Err(error("clipboard_changed")); }
                (content, after)
            };
            #[cfg(not(windows))]
            let (content, sequence) = {
                let content = match self.board.get_image() {
                    Ok(raw) => {
                        validate_clipboard_dimensions(raw.width.try_into().map_err(|_|error("clipboard_image_too_large"))?,raw.height.try_into().map_err(|_|error("clipboard_image_too_large"))?).map_err(error)?;
                        if self.raw.as_ref().is_some_and(|old| old.width==raw.width && old.height==raw.height && old.bytes==raw.bytes) { return Ok(()); }
                        let image=encode(raw.width as u32,raw.height as u32,&raw.bytes)?;
                        self.raw=Some(raw.to_owned_img()); (None,Some(image))
                    }
                    Err(arboard::Error::ContentNotAvailable) => {
                        self.raw=None;
                        match self.board.get_text() {
                            Ok(text) => { validate_clipboard_text(&text).map_err(error)?; (Some(text),None) },
                            Err(arboard::Error::ContentNotAvailable) => (None,None),
                            Err(_) => return Err(error("clipboard_unavailable")),
                        }
                    }
                    Err(_) => return Err(error("clipboard_unavailable")),
                };
                (content,None)
            };
            // Sequence changes count even when a user copies identical content.
            if self.content.as_ref()!=Some(&content) || self.sequence!=sequence { self.bump(); }
            self.content=Some(content); self.sequence=sequence; Ok(())
        }
        fn bump(&mut self) { self.revision=self.revision.wrapping_add(1).max(1); }
        pub(super) fn exchange(&mut self, operation:DesktopClipboardOperation)->io::Result<DesktopClipboardSnapshot> {
            self.refresh()?;
            let (text,image,expected) = match operation {
                DesktopClipboardOperation::Read {after} => {
                    let changed=after!=Some(self.revision);
                    let (text,image)=if changed {self.content.clone().unwrap_or_default()}else{(None,None)};
                    return Ok(DesktopClipboardSnapshot{revision:self.revision,changed,text,image});
                }
                DesktopClipboardOperation::Write{text} => (Some(text),None,None),
                DesktopClipboardOperation::WriteImage{image,expected_revision} => (None,Some(image),expected_revision),
            };
            if expected.is_some_and(|r|r!=self.revision){return Err(error("clipboard_changed"));}
            let content=(text,image);
            if self.content.as_ref()!=Some(&content) {
                #[cfg(windows)] self.board.write(content.0.as_deref(),content.1.as_ref(),self.sequence)?;
                #[cfg(not(windows))] {
                    if let Some(image)=&content.1 {
                        let raw=arboard::ImageData{width:image.width as usize,height:image.height as usize,bytes:decode(image)?.into_raw().into()};
                        self.board.set_image(raw.clone()).map_err(|_|error("clipboard_write_failed"))?;
                        self.raw=Some(raw);
                    }else if let Some(text)=&content.0{self.board.set_text(text.clone()).map_err(|_|error("clipboard_write_failed"))?; self.raw=None;}
                }
                self.bump(); self.content=Some(content);
                #[cfg(windows)] {self.sequence=crate::clipboard_windows::sequence();}
            }
            Ok(DesktopClipboardSnapshot{revision:self.revision,changed:false,text:None,image:None})
        }
    }
}
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
pub fn worker_main() -> i32 {
    use std::io::{Read,Write};
    #[cfg(target_os = "linux")] linux_environment::initialize();
    let mut state=None;
    let mut input=std::io::stdin().lock(); let mut output=std::io::stdout().lock();
    loop {
        let mut size=[0;4]; if input.read_exact(&mut size).is_err(){return 0;}
        let size=u32::from_le_bytes(size) as usize; if size>DESKTOP_CLIPBOARD_JSON_BYTES{return 1;}
        let mut bytes=vec![0;size];if input.read_exact(&mut bytes).is_err(){return 1;}
        let Ok(mut operation)=serde_json::from_slice::<DesktopClipboardOperation>(&bytes)else{return 1;};
        if let Some(image)=operation.image_mut(){
            if image.validate_header().is_err(){return 1;}
            image.data.resize(image.bytes as usize,0);if input.read_exact(&mut image.data).is_err(){return 1;}
        }
        if operation.validate().is_err(){return 1;}
        let result=(||{
            if state.is_none(){state=Some(native::State::new()?);}
            state.as_mut().unwrap().exchange(operation)
        })().map_err(|e:io::Error|e.to_string());
        let Ok(bytes)=serde_json::to_vec(&result)else{return 1;};
        if output.write_all(&(bytes.len() as u32).to_le_bytes()).and_then(|_|output.write_all(&bytes)).is_err(){return 1;}
        if let Ok(snapshot)=&result { if let Some(image)=&snapshot.image {if output.write_all(&image.data).is_err(){return 1;}} }
        if output.flush().is_err(){return 1;}
    }
}
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub fn worker_main() -> i32 { 1 }
