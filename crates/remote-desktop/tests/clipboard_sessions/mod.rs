use super::*;
use std::sync::{Mutex, atomic::{AtomicBool, AtomicU64}};

#[derive(Default)]
struct ClipboardBackend { video: Backend, text: Mutex<Option<String>>, revision: AtomicU64, fail: AtomicBool, calls: AtomicUsize }
impl DesktopBackend for ClipboardBackend {
    fn screens(&self) -> MediaFuture<'_, Vec<DesktopScreen>> { self.video.screens() }
    fn start(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopSource>> { self.video.start(config) }
    fn supports_clipboard(&self) -> bool { true }
    fn supports_input(&self) -> bool { self.video.supports_input() }
    fn input(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopInputSink>> { self.video.input(config) }
    fn clipboard(&self, _: DesktopVideoConfig, operation: DesktopClipboardOperation) -> MediaFuture<'_, DesktopClipboardSnapshot> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) { return Err(DesktopError::Rejected { code: DesktopErrorCode::MediaUnavailable, message: "clipboard occupied".into() }); }
            operation.validate().map_err(DesktopError::Protocol)?;
            let mut text = self.text.lock().unwrap();
            let revision = self.revision.load(Ordering::SeqCst).max(1);
            Ok(match operation {
                DesktopClipboardOperation::Read { after } => DesktopClipboardSnapshot { revision, changed: after != Some(revision), text: if after != Some(revision) { text.clone() } else { None } },
                DesktopClipboardOperation::Write { text: next } => {
                    *text = Some(next); self.revision.store(revision + 1, Ordering::SeqCst);
                    DesktopClipboardSnapshot { revision: revision + 1, changed: false, text: None }
                }
            })
        })
    }
}

#[tokio::test]
async fn unicode_and_large_text_cross_reliable_chunks_without_echo_or_media_changes() {
    let backend = Arc::new(ClipboardBackend::default());
    let global = DesktopHost::new(Some(backend.clone()));
    let server = Harness::new(&global).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT).await.unwrap()
        .with_clipboard_support(Some(DESKTOP_CLIPBOARD_VERSION));
    viewer.next_frame(WAIT).await.unwrap();
    let mut clipboard = viewer.clipboard_control();
    let first = clipboard.exchange(DesktopClipboardOperation::Read { after: None }).await.unwrap();
    assert!(first.changed && first.text.is_none());
    for text in ["中文与 emoji 🖥️\r\n第二行\nthird line".to_owned(), "界".repeat(300_000), String::new()] {
        let written = clipboard.exchange(DesktopClipboardOperation::Write { text: text.clone() }).await.unwrap();
        assert_eq!(backend.text.lock().unwrap().as_ref(), Some(&text));
        let echo = clipboard.exchange(DesktopClipboardOperation::Read { after: Some(written.revision) }).await.unwrap();
        assert!(!echo.changed && echo.text.is_none());
        let read = clipboard.exchange(DesktopClipboardOperation::Read { after: None }).await.unwrap();
        assert_eq!(read.text.as_ref(), Some(&text));
        viewer.next_frame(WAIT).await.unwrap();
        server.legacy().await;
    }
    let calls = backend.calls.load(Ordering::SeqCst);
    assert!(clipboard.exchange(DesktopClipboardOperation::Write { text: "a".repeat(DESKTOP_CLIPBOARD_MAX_BYTES + 1) }).await.is_err());
    assert_eq!(backend.calls.load(Ordering::SeqCst), calls);
    viewer.close().await.unwrap();
    assert!(clipboard.exchange(DesktopClipboardOperation::Read { after: None }).await.is_err());
}

#[tokio::test]
async fn clipboard_failures_old_peers_and_obsolete_generations_never_close_video() {
    let backend = Arc::new(ClipboardBackend::default());
    let global = DesktopHost::new(Some(backend.clone()));
    let server = Harness::new(&global).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT).await.unwrap();
    viewer.next_frame(WAIT).await.unwrap();
    assert!(!viewer.clipboard_control().is_supported());
    assert!(viewer.clipboard_control().exchange(DesktopClipboardOperation::Read { after: None }).await.is_err());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    viewer = viewer.with_clipboard_support(Some(DESKTOP_CLIPBOARD_VERSION));
    let mut old = viewer.clipboard_control();
    backend.fail.store(true, Ordering::SeqCst);
    assert!(old.exchange(DesktopClipboardOperation::Read { after: None }).await.is_err());
    viewer.next_frame(WAIT).await.unwrap(); server.legacy().await;
    backend.fail.store(false, Ordering::SeqCst);
    assert!(old.exchange(DesktopClipboardOperation::Read { after: None }).await.is_ok());
    let id = viewer.opened().config.screen.id.clone();
    viewer.select_screen(id).await.unwrap();
    viewer.next_frame(WAIT).await.unwrap();
    assert!(old.exchange(DesktopClipboardOperation::Read { after: None }).await.is_err());
    assert!(viewer.clipboard_control().exchange(DesktopClipboardOperation::Read { after: None }).await.is_ok());
    let outsider = Harness::new(&global).await;
    assert!(matches!(outsider.request(RemoteDesktopRequest::Clipboard { version: DESKTOP_CLIPBOARD_VERSION,
        session_id: viewer.opened().config.session_id, generation: viewer.opened().config.generation,
        request: DesktopClipboardRequest::Read { after: None } }).await,
        RemoteDesktopReply::Error { code: DesktopErrorCode::WrongOwner, .. }));
    viewer.next_frame(WAIT).await.unwrap(); server.legacy().await;
    viewer.close().await.unwrap();
}
