use super::*;
use pab_desktop_media::clipboard::NativeClipboard;
use pab_protocol::{DesktopClipboardOperation as Operation, DesktopClipboardSnapshot as Snapshot};
use pab_remote_desktop::RemoteDesktopClipboardControl;
use std::sync::Mutex as StdMutex;

// Only the last focused/operated viewer owns the local clipboard across windows.
static ACTIVE: StdMutex<Option<String>> = StdMutex::new(None);
struct Progress { remote: RemoteDesktopClipboardControl, local_revision: Option<u64>, remote_revision: Option<u64> }
pub(super) struct Bridge<'a> {
    window: &'a WebviewWindow,
    owner: &'a Owner,
    generation: InputGeneration,
    token: String,
    local: NativeClipboard,
    progress: Mutex<Progress>,
    notify: tokio::sync::Notify,
    supported: bool,
}
impl Drop for Bridge<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE.lock() { if active.as_ref() == Some(&self.token) { *active = None; } }
    }
}
impl<'a> Bridge<'a> {
    pub(super) fn new(window: &'a WebviewWindow, owner: &'a Owner, generation: InputGeneration, remote: RemoteDesktopClipboardControl) -> Result<Self, String> {
        let supported = remote.is_supported();
        Ok(Self { window, owner, generation, token: format!("{}:{}", window.label(), generation.generation),
            local: NativeClipboard::new(std::env::current_exe().map_err(|_| "clipboard worker unavailable")?, vec!["--media-worker".into()]),
            progress: Mutex::new(Progress { remote, local_revision: None, remote_revision: None }), notify: tokio::sync::Notify::new(), supported })
    }
    pub(super) fn activate(&self) { if let Ok(mut active) = ACTIVE.lock() { *active = Some(self.token.clone()); } }
    pub(super) fn deactivate(&self) {
        if let Ok(mut active)=ACTIVE.lock(){if active.as_ref()==Some(&self.token){*active=None;}}
    }
    pub(super) fn paste_allowed(&self)->bool { self.enabled() && self.owns_clipboard() }
    async fn guarded<T>(&self, work:impl std::future::Future<Output=Result<T,String>>)->Result<T,String> {
        tokio::select! {
            result=work=>result,
            _=async {while self.paste_allowed(){tokio::time::sleep(Duration::from_millis(30)).await;}}=>Err("clipboard_cancelled".into()),
        }
    }
    fn owns_clipboard(&self) -> bool { ACTIVE.lock().is_ok_and(|active| active.as_ref() == Some(&self.token)) }
    fn enabled(&self) -> bool {
        let status = self.owner.status.borrow();
        !*self.owner.stop.borrow() && self.owner.clipboard_requested.load(Ordering::Acquire) && status.input_enabled
            && status.phase == "viewing" && status.opened.as_ref().is_some_and(|opened| InputGeneration::from(opened) == self.generation)
    }
    fn publish(&self, state: &'static str) {
        let changed = self.owner.status.send_if_modified(|status| {
            if status.clipboard_state == state || !status.opened.as_ref().is_some_and(|opened| InputGeneration::from(opened) == self.generation) { return false; }
            status.clipboard_state = state;
            true
        });
        if changed {
            let status = self.owner.status.borrow().clone();
            let _ = self.window.emit_to(self.window.label(), "desktop-viewer-status", status);
        }
    }
    pub(super) fn copied(&self) { self.notify.notify_one(); }
    pub(super) async fn run(&self) {
        let mut timer = tokio::time::interval(Duration::from_millis(333));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut retry_after = Instant::now();
        loop {
            tokio::select! {
                _ = timer.tick() => {},
                _ = self.notify.notified() => tokio::time::sleep(Duration::from_millis(60)).await,
            }
            if !self.owner.clipboard_requested.load(Ordering::Acquire) {
                let mut progress = self.progress.lock().await; progress.local_revision = None; progress.remote_revision = None;
                self.publish("disabled"); continue;
            }
            if !self.supported { self.publish("unsupported"); continue; }
            if !self.enabled() {
                let mut progress = self.progress.lock().await;
                progress.local_revision = None; progress.remote_revision = None;
                self.publish("waiting"); continue;
            }
            if self.window.is_focused().unwrap_or(false) { self.activate(); }
            if !self.owns_clipboard() || Instant::now() < retry_after { continue; }
            match self.guarded(self.tick()).await {
                Ok(())=>self.publish("active"),
                Err(error)=>{self.publish(if error.contains("too_large"){"too_large"}else{"unavailable"}); retry_after=Instant::now()+Duration::from_secs(3);}
            }
        }
    }
    async fn tick(&self) -> Result<(), String> {
        let mut progress=self.progress.lock().await;
        let local=self.local.exchange(Operation::Read{after:progress.local_revision}).await.map_err(|e|e.to_string())?;
        if progress.remote_revision.is_none(){
            let remote=progress.remote.exchange(Operation::Read{after:None}).await.map_err(|e|e.to_string())?;
            progress.local_revision=Some(local.revision);progress.remote_revision=Some(remote.revision);return Ok(());
        }
        if !self.paste_allowed(){return Ok(());}
        if local.changed {
            if let Some(operation)=write_operation(&local, progress.remote_revision){
                self.publish("syncing");
                let remote=match transfer(&mut progress.remote,operation).await {
                    Ok(remote)=>remote,
                    Err(error)=>{
                        if error.to_string().contains("clipboard_changed") {progress.local_revision=None;progress.remote_revision=None;}
                        return Err(error.to_string());
                    }
                };
                progress.remote_revision=Some(remote.revision);
            }else{
                let after=progress.remote_revision;
                let remote=progress.remote.exchange(Operation::Read{after}).await.map_err(|e|e.to_string())?;
                progress.remote_revision=Some(remote.revision);
            }
            progress.local_revision=Some(local.revision);return Ok(());
        }
        let after=progress.remote_revision;
        let remote=progress.remote.exchange(Operation::Read{after}).await.map_err(|e|e.to_string())?;
        if let Some(operation)=write_operation(&remote,progress.local_revision){
            let current=self.local.exchange(Operation::Read{after:progress.local_revision}).await.map_err(|e|e.to_string())?;
            if current.changed || !self.paste_allowed(){return Ok(());}
            self.publish("syncing");
            let applied=self.local.exchange(operation).await.map_err(|e|e.to_string())?;
            progress.local_revision=Some(applied.revision);
        }
        progress.remote_revision=Some(remote.revision);Ok(())
    }
    pub(super) async fn before_paste(&self) -> bool {
        if !self.owner.clipboard_requested.load(Ordering::Acquire) || !self.supported{return true;}
        self.publish("syncing");
        let result=self.guarded(async {
            // Capture what was copied before waiting for an in-flight poll. A
            // background remote update must not silently replace this paste.
            let local=self.local.exchange(Operation::Read{after:None}).await.map_err(|e|e.to_string())?;
            let mut progress=self.progress.lock().await;
            let current=self.local.exchange(Operation::Read{after:Some(local.revision)}).await.map_err(|e|e.to_string())?;
            if current.changed || !self.paste_allowed(){return Err("clipboard_cancelled".into());}
            // Explicit paste intentionally replaces the remote clipboard. Background
            // propagation uses expected_revision to preserve a newer remote copy.
            if let Some(operation)=write_operation(&local,None){
                let already_synced=if progress.local_revision==Some(local.revision) && progress.remote_revision.is_some(){
                    let after=progress.remote_revision;
                    !progress.remote.exchange(Operation::Read{after}).await.map_err(|e|e.to_string())?.changed
                }else{false};
                if !already_synced{
                    let remote=transfer(&mut progress.remote,operation).await?;
                    progress.remote_revision=Some(remote.revision);
                }
                let current=self.local.exchange(Operation::Read{after:Some(local.revision)}).await.map_err(|e|e.to_string())?;
                if current.changed{return Err("clipboard_changed".into());}
            }else{return Err("clipboard_content_unavailable".into());}
            progress.local_revision=Some(local.revision);Ok(())
        }).await;
        self.publish(match &result{Ok(())=>"active",Err(e) if e.contains("too_large")=>"too_large",Err(_)=>"paste_failed"});
        result.is_ok()
    }

}

async fn transfer(remote:&mut RemoteDesktopClipboardControl,operation:Operation)->Result<Snapshot,String>{
    let image=operation.image().map(|i|(i.width,i.height,i.bytes));
    let started=Instant::now();
    let result=remote.exchange(operation).await.map_err(|e|e.to_string());
    if let Some((width,height,bytes))=image{
        tracing::info!(width,height,bytes,elapsed_ms=started.elapsed().as_millis() as u64,success=result.is_ok(),
            error=result.as_ref().err().map(String::as_str),"remote desktop image clipboard write");
    }
    result
}
fn write_operation(snapshot:&Snapshot,expected_revision:Option<u64>)->Option<Operation>{
    if let Some(image)=&snapshot.image{Some(Operation::WriteImage{image:image.clone(),expected_revision})}
    else{snapshot.text.clone().map(|text|Operation::Write{text})}
}
#[path = "clipboard_shortcuts.rs"]
mod shortcuts;
pub(super) use shortcuts::Shortcuts;
