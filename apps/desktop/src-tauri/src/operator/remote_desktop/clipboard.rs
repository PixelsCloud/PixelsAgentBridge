use super::*;
use pab_desktop_media::clipboard::NativeClipboard;
use pab_protocol::{DesktopClipboardOperation as Operation, RemoteDesktopInput};
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
            if self.tick().await.is_err() {
                self.publish("unavailable"); retry_after = Instant::now() + Duration::from_secs(3);
            } else { self.publish("active"); }
        }
    }
    async fn tick(&self) -> Result<(), ()> {
        let mut progress = self.progress.lock().await;
        let local = self.local.exchange(Operation::Read { after: progress.local_revision }).await.map_err(|_| ())?;
        if progress.remote_revision.is_none() {
            let remote = progress.remote.exchange(Operation::Read { after: None }).await.map_err(|_| ())?;
            // Starting a viewer never replaces either side's existing clipboard.
            progress.local_revision = Some(local.revision); progress.remote_revision = Some(remote.revision);
            return Ok(());
        }
        if !self.enabled() || !self.owns_clipboard() { return Ok(()); }
        if local.changed {
            if let Some(text) = local.text {
                let remote = progress.remote.exchange(Operation::Write { text }).await.map_err(|_| ())?;
                progress.remote_revision = Some(remote.revision);
            } else {
                // A newly copied image/file stays local; do not replace it with old text.
                let after = progress.remote_revision;
                let remote = progress.remote.exchange(Operation::Read { after }).await.map_err(|_| ())?;
                progress.remote_revision = Some(remote.revision);
            }
            progress.local_revision = Some(local.revision);
            return Ok(());
        }
        let after = progress.remote_revision;
        let remote = progress.remote.exchange(Operation::Read { after }).await.map_err(|_| ())?;
        if let Some(text) = remote.text {
            let current = self.local.exchange(Operation::Read { after: progress.local_revision }).await.map_err(|_| ())?;
            // A copy made locally during the network read wins over the older reply.
            if current.changed || !self.enabled() || !self.owns_clipboard() { return Ok(()); }
            let applied = self.local.exchange(Operation::Write { text }).await.map_err(|_| ())?;
            progress.local_revision = Some(applied.revision);
        }
        progress.remote_revision = Some(remote.revision);
        Ok(())
    }
    pub(super) async fn before_paste(&self) -> bool {
        if !self.owner.clipboard_requested.load(Ordering::Acquire) { return true; }
        if !self.supported { return true; } // An old peer keeps its existing remote-only paste behavior.
        self.activate();
        let result = async {
            let mut progress = self.progress.lock().await;
            let local = self.local.exchange(Operation::Read { after: None }).await.map_err(|_| ())?;
            if !self.enabled() { return Err(()); }
            if let Some(text) = local.text {
                let remote = progress.remote.exchange(Operation::Write { text }).await.map_err(|_| ())?;
                progress.remote_revision = Some(remote.revision);
            }
            progress.local_revision = Some(local.revision);
            Ok::<_, ()>(())
        }.await;
        self.publish(if result.is_ok() { "active" } else { "paste_failed" });
        result.is_ok()
    }
}

#[derive(Default)]
pub(super) struct Shortcuts { held: std::collections::HashSet<u16> }
impl Shortcuts {
    pub(super) fn observe(&mut self, event: &RemoteDesktopInput) -> (bool, bool) {
        match event {
            RemoteDesktopInput::ReleaseAll => { self.held.clear(); (false, false) }
            RemoteDesktopInput::Key { virtual_key, down } => {
                let shortcut = [0x11, 0xa2, 0xa3, 0x5b, 0x5c].iter().any(|key| self.held.contains(key));
                let shift = [0x10, 0xa0, 0xa1].iter().any(|key| self.held.contains(key));
                let paste = *down && ((*virtual_key == 0x56 && shortcut) || (*virtual_key == 0x2d && shift));
                let copy = !*down && shortcut && matches!(*virtual_key, 0x43 | 0x58);
                if *down { self.held.insert(*virtual_key); } else { self.held.remove(virtual_key); }
                (paste, copy)
            }
            _ => (false, false),
        }
    }
}
