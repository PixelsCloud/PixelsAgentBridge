use super::*;
use pab_desktop_media::file_clipboard::NativeFileClipboard;
use pab_protocol::{ClipboardFileRead, FileClipboardOperation as Op, RequestId};
pub(super) struct Files {
    pub supported: bool,
    local: NativeFileClipboard,
    progress: Mutex<Progress>,
}
struct Progress {
    remote: RemoteDesktopClipboardControl,
    local_sequence: Option<u64>,
    remote_sequence: Option<u64>,
    exported: Option<RequestId>,
    imported: Option<RequestId>,
    active: bool,
    local_files: bool,
    remote_files: bool,
    failed: bool,
}
impl Files {
    pub(super) fn new(remote: RemoteDesktopClipboardControl) -> Result<Self, String> {
        Ok(Self {
            supported: cfg!(windows) && remote.files_supported(),
            local: NativeFileClipboard::new(
                std::env::current_exe().map_err(|_| "file_clipboard_unavailable")?,
                vec!["--media-worker".into()],
            ),
            progress: Mutex::new(Progress {
                remote,
                local_sequence: None,
                remote_sequence: None,
                exported: None,
                imported: None,
                active: false,
                local_files: false,
                remote_files: false,
                failed: false,
            }),
        })
    }
}
impl Bridge<'_> {
    pub(super) fn publish_files(&self, state: &'static str) {
        if self.owner.status.send_if_modified(|status| {
            if status.file_clipboard_state == state
                || !status
                    .opened
                    .as_ref()
                    .is_some_and(|v| InputGeneration::from(v) == self.generation)
            {
                return false;
            }
            status.file_clipboard_state = state;
            true
        }) {
            let status = self.owner.status.borrow().clone();
            let _ = self
                .window
                .emit_to(self.window.label(), "desktop-viewer-status", status);
        }
    }
    pub(in crate::operator::remote_desktop) async fn clear_files(&self) {
        if !self.files.supported {
            return;
        }
        tracing::info!(
            window = self.window.label(),
            "file clipboard clear requested"
        );
        let mut p = self.files.progress.lock().await;
        let _ = self.files.local.exchange(Op::Clear).await;
        let _ = p.remote.files(Op::Clear).await;
        p.local_sequence = None;
        p.remote_sequence = None;
        p.exported = None;
        p.imported = None;
        p.local_files = false;
        p.remote_files = false;
        p.active = false;
        p.failed = false;
    }
    async fn deliver_files(
        &self,
        p: &mut Progress,
        requests: Vec<ClipboardFileRead>,
        local_target: bool,
    ) -> Result<(), String> {
        for request in requests {
            let ended = request.end || request.batch_event;
            let id = request.id;
            let result = if local_target {
                p.remote
                    .files(Op::Read { request })
                    .await
                    .map_err(|e| e.to_string())
            } else {
                self.files
                    .local
                    .exchange(Op::Read { request })
                    .await
                    .map_err(|e| e.to_string())
            };
            if ended {
                continue;
            }
            let response = match result {
                Ok(snapshot) => Op::Deliver {
                    id,
                    bytes: snapshot.bytes,
                    error: None,
                    data: snapshot.data,
                },
                Err(_) => {
                    p.failed = true;
                    Op::Deliver {
                        id,
                        bytes: 0,
                        error: Some("file_clipboard_read_failed".into()),
                        data: vec![],
                    }
                }
            };
            if local_target {
                self.files
                    .local
                    .exchange(response)
                    .await
                    .map_err(|e| e.to_string())?;
            } else {
                p.remote.files(response).await.map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
    pub(super) async fn file_tick(&self, active: bool) -> Result<bool, String> {
        let mut p = self.files.progress.lock().await;
        let just_activated = active && !p.active;
        p.active = active;
        let after = if just_activated {
            None
        } else {
            p.local_sequence
        };
        let mut local = self
            .files
            .local
            .exchange(Op::Poll { after })
            .await
            .map_err(|e| e.to_string())?;
        let after = if just_activated {
            None
        } else {
            p.remote_sequence
        };
        let mut remote = p
            .remote
            .files(Op::Poll { after })
            .await
            .map_err(|e| e.to_string())?;
        if just_activated || local.changed || remote.changed {
            tracing::info!(
                active,
                just_activated,
                local_changed = local.changed,
                local_offer = local.offer.is_some(),
                local_imported = local.imported,
                remote_changed = remote.changed,
                remote_offer = remote.offer.is_some(),
                remote_imported = remote.imported,
                "file clipboard selection observed"
            );
        }
        p.local_sequence = Some(local.sequence);
        p.remote_sequence = Some(remote.sequence);
        let local_new = local.changed && local.offer.is_some() && !local.imported;
        let remote_new = remote.changed && remote.offer.is_some() && !remote.imported;
        let remote_plain = active
            && remote.changed
            && !remote.imported
            && remote.offer.is_none()
            && !local_new
            && p.exported.is_some();
        if remote_plain {
            // A remote text/image copy supersedes our older file selection.
            // Otherwise local_files would suppress the text pump indefinitely.
            p.local_files = false;
        }
        if local.changed && !local.imported {
            p.imported = None;
        }
        if remote.changed && !remote.imported {
            p.exported = None;
        }
        if local.changed {
            p.local_files = local.offer.is_some();
        }
        if remote.changed {
            p.remote_files = remote.offer.is_some();
        }
        if local_new || remote_new {
            p.failed = false;
        }
        // Read leases remain tied to their device even when another window owns
        // the clipboard. Focus changes only gate new advertisements.
        self.deliver_files(&mut p, std::mem::take(&mut local.requests), true)
            .await?;
        self.deliver_files(&mut p, std::mem::take(&mut remote.requests), false)
            .await?;
        if local.changed && !local.imported && !(active && local_new) {
            if let Some(offer) = p.exported.take() {
                let applied = p
                    .remote
                    .files(Op::Withdraw { offer })
                    .await
                    .map_err(|e| e.to_string())?;
                p.remote_sequence = Some(applied.sequence);
            }
        }
        if remote.changed
            && !remote.imported
            && !(active && remote_new && (!local.changed || just_activated || local.imported))
        {
            if let Some(offer) = p.imported.take() {
                let applied = self
                    .files
                    .local
                    .exchange(Op::Withdraw { offer })
                    .await
                    .map_err(|e| e.to_string())?;
                p.local_sequence = Some(applied.sequence);
            }
        }
        // Polling either peer can yield while another viewer becomes active.
        // Keep draining existing read leases, but never publish a new selection
        // using authority captured before those awaits.
        if active && self.paste_allowed() {
            if local_new {
                let offer = local.offer.take().unwrap();
                let id = offer.id;
                self.publish_files("preparing");
                let applied = p
                    .remote
                    .files(Op::Publish {
                        offer,
                        expected_sequence: Some(remote.sequence),
                    })
                    .await
                    .map_err(|e| {
                        tracing::warn!(error=%e,"file clipboard advertisement failed");
                        e.to_string()
                    })?;
                p.exported = Some(id);
                tracing::info!(
                    imported = applied.imported,
                    sequence = applied.sequence,
                    "file clipboard advertisement acknowledged"
                );
                p.imported = None;
                p.remote_sequence = Some(applied.sequence);
                p.remote_files = false;
            } else if remote_new && (!local.changed || just_activated || local.imported) {
                let offer = remote.offer.take().unwrap();
                let id = offer.id;
                let applied = self
                    .files
                    .local
                    .exchange(Op::Publish {
                        offer,
                        expected_sequence: Some(local.sequence),
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                p.imported = Some(id);
                tracing::info!(
                    imported = applied.imported,
                    sequence = applied.sequence,
                    "local file clipboard advertisement acknowledged"
                );
                p.exported = None;
                p.local_sequence = Some(applied.sequence);
                p.local_files = false;
            }
        }
        if remote_plain && self.paste_allowed() {
            self.resume_plain_clipboard(true).await?;
        }
        // Do not replace the revision returned by our own Publish/Withdraw with
        // the older observation taken before that write.
        let present = p.local_files
            || p.imported.is_some()
            || p.exported.is_some()
            || local.error.is_some()
            || remote.error.is_some();
        if let Some(error) = local.error.or(remote.error) {
            self.publish_files(if error.contains("cut_unsupported") {
                "cut_unsupported"
            } else {
                "source_unavailable"
            });
        } else {
            self.publish_files(if p.failed {
                "read_failed"
            } else if p.exported.is_some() || p.imported.is_some() {
                "ready"
            } else if present {
                "preparing"
            } else {
                "active"
            });
        }
        Ok(present)
    }
    pub(super) async fn resume_plain_clipboard(&self, prefer_remote: bool) -> Result<(), String> {
        // Acknowledge a content revision only after applying it. This also
        // refreshes the image CAS revision after a file clipboard publication.
        let mut text = self.progress.lock().await;
        let local = self
            .local
            .exchange(Operation::Read { after: None })
            .await
            .map_err(|e| e.to_string())?;
        let remote = text
            .remote
            .exchange(Operation::Read { after: None })
            .await
            .map_err(|e| e.to_string())?;
        let current = self
            .local
            .exchange(Operation::Read {
                after: Some(local.revision),
            })
            .await
            .map_err(|e| e.to_string())?;
        if current.changed || !self.paste_allowed() {
            return Err("clipboard_changed".into());
        }
        if !prefer_remote && text.local_revision != Some(local.revision) {
            if let Some(operation) = write_operation(&local, Some(remote.revision)) {
                let applied = transfer(&mut text.remote, operation).await?;
                text.local_revision = Some(local.revision);
                text.remote_revision = Some(applied.revision);
                return Ok(());
            }
        }
        if let Some(operation) = write_operation(&remote, Some(local.revision)) {
            let applied = self
                .local
                .exchange(operation)
                .await
                .map_err(|e| e.to_string())?;
            text.local_revision = Some(applied.revision);
            text.remote_revision = Some(remote.revision);
        }
        Ok(())
    }
    pub(super) async fn file_before_paste(&self) -> Option<bool> {
        if !cfg!(windows) {
            return None;
        }
        if !self.files.supported {
            match self.files.local.exchange(Op::Poll { after: None }).await {
                Ok(snapshot) if snapshot.offer.is_some() || snapshot.error.is_some() => {
                    self.publish_files("unsupported_files");
                    return Some(false);
                }
                _ => return None,
            }
        }
        // Explicit paste rechecks the current selection, including a prior
        // advertisement rejected because the other clipboard changed.
        self.files.progress.lock().await.active = false;
        match self.guarded(self.file_tick(true)).await {
            Ok(true) => {
                let p = self.files.progress.lock().await;
                Some(p.exported.is_some() || p.imported.is_some())
            }
            Ok(false) => {
                let snapshot = self
                    .files
                    .local
                    .exchange(Op::Poll { after: None })
                    .await
                    .ok()?;
                if snapshot.error.is_some() {
                    Some(false)
                } else {
                    None
                }
            }
            Err(_) => {
                self.file_failed().await;
                self.publish_files("unavailable");
                Some(false)
            }
        }
    }
    pub(super) async fn file_failed(&self) {
        let mut p = self.files.progress.lock().await;
        if p.local_files || p.exported.is_some() || p.imported.is_some() {
            p.failed = true;
        }
    }
}
