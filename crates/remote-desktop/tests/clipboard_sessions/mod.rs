use super::*;
use std::sync::{Mutex, atomic::{AtomicBool, AtomicU64}};

#[derive(Default)]
struct ClipboardBackend { video: Backend, text: Mutex<Option<String>>, image:Mutex<Option<DesktopClipboardImage>>, revision: AtomicU64, fail: AtomicBool, calls: AtomicUsize }
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
                DesktopClipboardOperation::Read { after } => DesktopClipboardSnapshot { revision, changed: after != Some(revision), text: if after != Some(revision) { text.clone() } else { None }, image:if after!=Some(revision){self.image.lock().unwrap().clone()}else{None} },
                DesktopClipboardOperation::WriteImage{image,expected_revision}=>{
                    if expected_revision.is_some_and(|v|v!=revision){return Err(DesktopError::Protocol("clipboard_changed"));}
                    *text=None;*self.image.lock().unwrap()=Some(image);self.revision.store(revision+1,Ordering::SeqCst);
                    DesktopClipboardSnapshot{revision:revision+1,changed:false,text:None,image:None}
                }
                DesktopClipboardOperation::Write { text: next } => {
                    *text = Some(next); *self.image.lock().unwrap()=None; self.revision.store(revision + 1, Ordering::SeqCst);
                    DesktopClipboardSnapshot { revision: revision + 1, changed: false, text: None, image:None }
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

// The transport treats PNG data as opaque; native decoder bounds/CRC have their
// own tests. Deliberately exceed multiple frames without a codec dependency.
fn image_payload()->DesktopClipboardImage{
    let mut data=vec![37;512*1024];
    data[..16].copy_from_slice(&[137,80,78,71,13,10,26,10,0,0,0,13,73,72,68,82]);
    data[16..20].copy_from_slice(&2u32.to_be_bytes());data[20..24].copy_from_slice(&2u32.to_be_bytes());
    DesktopClipboardImage{width:2,height:2,bytes:data.len() as u32,data}
}
#[tokio::test]
async fn image_binary_roundtrip_no_echo_cas_and_rejected_header_keep_video_alive(){
    let backend=Arc::new(ClipboardBackend::default());
    let global=DesktopHost::new(Some(backend.clone()));let server=Harness::new(&global).await;
    let mut viewer=RemoteDesktopSession::open(server.client.clone(),None,15,WAIT).await.unwrap().with_clipboard_support(Some(DESKTOP_CLIPBOARD_VERSION));
    viewer.next_frame(WAIT).await.unwrap();let mut clipboard=viewer.clipboard_control();
    let image=image_payload();
    assert!(serde_json::to_vec(&image).unwrap().len()<100);
    let written=clipboard.exchange(DesktopClipboardOperation::WriteImage{image:image.clone(),expected_revision:Some(1)}).await.unwrap();
    let read=clipboard.exchange(DesktopClipboardOperation::Read{after:None}).await.unwrap();assert_eq!(read.image,Some(image.clone()));assert!(read.text.is_none());
    let echo=clipboard.exchange(DesktopClipboardOperation::Read{after:Some(written.revision)}).await.unwrap();assert!(!echo.changed && echo.image.is_none());
    assert!(clipboard.exchange(DesktopClipboardOperation::WriteImage{image:image.clone(),expected_revision:Some(1)}).await.is_err());
    let mut bad=image;bad.width=16384;bad.height=16384;
    assert!(clipboard.exchange(DesktopClipboardOperation::WriteImage{image:bad,expected_revision:None}).await.is_err());
    viewer.next_frame(WAIT).await.unwrap();server.legacy().await;viewer.close().await.unwrap();
}
