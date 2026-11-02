use super::*;
use sha2::{Digest, Sha256};
use std::sync::Mutex;
#[derive(Default)]
struct FilesBackend {
    video: Backend,
    offer: Mutex<Option<ClipboardFileOffer>>,
    delivered: Mutex<Vec<u8>>,
    calls: AtomicUsize,
}
impl DesktopBackend for FilesBackend {
    fn supports_input(&self) -> bool {
        true
    }
    fn input(&self, c: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopInputSink>> {
        self.video.input(c)
    }
    fn screens(&self) -> MediaFuture<'_, Vec<DesktopScreen>> {
        self.video.screens()
    }
    fn start(&self, c: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopSource>> {
        self.video.start(c)
    }
    fn supports_file_clipboard(&self) -> bool {
        true
    }
    fn file_clipboard(
        &self,
        _: DesktopVideoConfig,
        operation: FileClipboardOperation,
    ) -> MediaFuture<'_, FileClipboardSnapshot> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match operation {
                FileClipboardOperation::Publish { offer, .. } => {
                    *self.offer.lock().unwrap() = Some(offer);
                    Ok(FileClipboardSnapshot::default())
                }
                FileClipboardOperation::Poll { .. } => Ok(FileClipboardSnapshot {
                    offer: self.offer.lock().unwrap().clone(),
                    sequence: 1,
                    changed: true,
                    ..Default::default()
                }),
                FileClipboardOperation::Read { request } => {
                    let data = (0..request.length as u64)
                        .map(|i| ((request.offset + i) % 251) as u8)
                        .collect::<Vec<_>>();
                    Ok(FileClipboardSnapshot {
                        bytes: data.len() as u32,
                        sha256: Some(format!("{:x}", Sha256::digest(&data))),
                        data,
                        ..Default::default()
                    })
                }
                FileClipboardOperation::Deliver { data, .. } => {
                    *self.delivered.lock().unwrap() = data;
                    Ok(FileClipboardSnapshot::default())
                }
                _ => Ok(FileClipboardSnapshot::default()),
            }
        })
    }
}
#[tokio::test]
async fn file_clipboard_sessions_fragmented_manifest_binary_ranges_and_generation_authority() {
    let backend = Arc::new(FilesBackend::default());
    let global = DesktopHost::new(Some(backend.clone()));
    let server = Harness::new(&global).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 30, WAIT)
        .await
        .unwrap();
    viewer.next_frame(WAIT).await.unwrap();
    assert!(
        viewer
            .clipboard_control()
            .files(FileClipboardOperation::Poll { after: None })
            .await
            .is_err()
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    viewer = viewer.with_file_clipboard_support(Some(FILE_CLIPBOARD_VERSION));
    let mut files = viewer.clipboard_control();
    let offer = ClipboardFileOffer {
        id: RequestId::new(),
        files: (0..1800)
            .map(|i| ClipboardFile {
                name: format!("file_{i:04}_{}.bin", "x".repeat(170)),
                directory: false,
                size: 6 * 1024 * 1024 * 1024,
                modified: 0,
            })
            .collect(),
    };
    offer.validate().unwrap();
    assert!(serde_json::to_vec(&offer).unwrap().len() > MAX_BINARY_FRAME_BYTES);
    files
        .files(FileClipboardOperation::Publish {
            offer: offer.clone(),
            expected_sequence: None,
        })
        .await
        .unwrap();
    assert_eq!(
        files
            .files(FileClipboardOperation::Poll { after: None })
            .await
            .unwrap()
            .offer,
        Some(offer.clone())
    );
    let offset = 5 * 1024 * 1024 * 1024 + 17;
    let snapshot = files
        .files(FileClipboardOperation::Read {
            request: ClipboardFileRead {
                id: 1,
                stream: 1,
                batch: 1,
                batch_event: false,
                offer: offer.id,
                index: 0,
                offset,
                length: FILE_CLIPBOARD_CHUNK as u32,
                end: false,
            },
        })
        .await
        .unwrap();
    let expected = (0..FILE_CLIPBOARD_CHUNK as u64)
        .map(|i| ((offset + i) % 251) as u8)
        .collect::<Vec<_>>();
    assert_eq!(snapshot.data, expected);
    files
        .files(FileClipboardOperation::Deliver {
            id: 1,
            bytes: snapshot.bytes,
            error: None,
            data: snapshot.data,
        })
        .await
        .unwrap();
    assert_eq!(*backend.delivered.lock().unwrap(), expected);
    let outsider = Harness::new(&global).await;
    assert!(matches!(
        outsider
            .request(RemoteDesktopRequest::FileClipboard {
                version: FILE_CLIPBOARD_VERSION,
                session_id: viewer.opened().config.session_id,
                generation: viewer.opened().config.generation,
                header_bytes: 2,
                payload_bytes: 0
            })
            .await,
        RemoteDesktopReply::Error {
            code: DesktopErrorCode::WrongOwner,
            ..
        }
    ));
    let screen = viewer.opened().config.screen.id.clone();
    viewer.select_screen(screen).await.unwrap();
    viewer.next_frame(WAIT).await.unwrap();
    let before = backend.calls.load(Ordering::SeqCst);
    assert!(
        files
            .files(FileClipboardOperation::Poll { after: None })
            .await
            .is_err()
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), before);
    viewer
        .clipboard_control()
        .files(FileClipboardOperation::Poll { after: None })
        .await
        .unwrap();
    server.legacy().await;
    viewer.close().await.unwrap();
}
