use super::{Backend, Harness, WAIT, eventually};
use pab_protocol::*;
use pab_remote_desktop::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

#[derive(Default)]
struct CursorBackend {
    video: Backend,
    active: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
    produced: Arc<AtomicUsize>,
    wake: Arc<Notify>,
    fail: bool,
    pending_start: bool,
}
struct Source {
    config: DesktopVideoConfig,
    active: Arc<AtomicUsize>,
    produced: Arc<AtomicUsize>,
    wake: Arc<Notify>,
    sequence: u64,
    fail: bool,
}
impl Drop for Source {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}
impl DesktopCursorSource for Source {
    fn next_update(&mut self) -> MediaFuture<'_, CursorUpdate> {
        Box::pin(async move {
            if self.sequence > 0 {
                if self.fail {
                    return Err(DesktopError::Protocol("injected cursor failure"));
                }
                self.wake.notified().await;
            }
            self.sequence += 1;
            self.produced.fetch_add(1, Ordering::SeqCst);
            Ok(CursorUpdate {
                header: DesktopCursorHeader {
                    version: DESKTOP_CURSOR_SCHEMA_VERSION,
                    session_id: self.config.session_id,
                    generation: self.config.generation,
                    sequence: self.sequence,
                    position: Some(DesktopCursorPosition {
                        x: self.config.screen.x + self.sequence as i32,
                        y: self.config.screen.y,
                    }),
                    image_id: 1,
                    image: (self.sequence == 1).then_some(DesktopCursorImage {
                        format: DesktopCursorFormat::PremultipliedBgra,
                        width: 1,
                        height: 1,
                        hotspot_x: 0,
                        hotspot_y: 0,
                    }),
                },
                bytes: if self.sequence == 1 {
                    vec![0, 0, 255, 255]
                } else {
                    vec![]
                },
            })
        })
    }
}
impl DesktopBackend for CursorBackend {
    fn supports_cursor(&self) -> bool {
        true
    }
    fn supports_input(&self) -> bool {
        self.video.supports_input()
    }
    fn screens(&self) -> MediaFuture<'_, Vec<DesktopScreen>> {
        self.video.screens()
    }
    fn start(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopSource>> {
        self.video.start(config)
    }
    fn input(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopInputSink>> {
        self.video.input(config)
    }
    fn cursor(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopCursorSource>> {
        Box::pin(async move {
            let count = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(count, Ordering::SeqCst);
            let source = Source {
                config,
                active: self.active.clone(),
                produced: self.produced.clone(),
                wake: self.wake.clone(),
                sequence: 0,
                fail: self.fail,
            };
            if self.pending_start {
                std::future::pending::<()>().await;
            }
            Ok(Box::new(source) as Box<dyn DesktopCursorSource>)
        })
    }
}
async fn viewer(h: &Harness) -> RemoteDesktopSession {
    RemoteDesktopSession::open(h.client.clone(), None, 15, WAIT)
        .await
        .unwrap()
        .with_cursor_support(Some(DESKTOP_CURSOR_SCHEMA_VERSION))
}
fn request(viewer: &RemoteDesktopSession, version: u16) -> RemoteDesktopRequest {
    RemoteDesktopRequest::Cursor {
        version,
        session_id: viewer.opened().config.session_id,
        generation: viewer.opened().config.generation,
    }
}

#[tokio::test]
async fn cursor_updates_during_static_video_and_exhausted_video_credit() {
    for stationary in [false, true] {
        let backend = Arc::new(CursorBackend {
            video: Backend {
                wake: stationary.then(|| Arc::new(Notify::new())),
                ..Default::default()
            },
            ..Default::default()
        });
        let global = DesktopHost::new(Some(backend.clone()));
        let h = Harness::new(&global).await;
        let mut viewer = viewer(&h).await;
        let frames = if stationary { 1 } else { DESKTOP_MAX_IN_FLIGHT };
        for _ in 0..frames {
            viewer.next_frame(WAIT).await.unwrap();
        }
        assert_eq!(
            backend.video.produced.load(Ordering::SeqCst),
            frames as usize
        );
        let mut cursor = viewer.open_cursor().await.unwrap();
        assert!(cursor.is_current());
        assert_eq!(cursor.next_update().await.unwrap().bytes, [0, 0, 255, 255]);
        backend.wake.notify_one();
        let moved = tokio::time::timeout(WAIT, cursor.next_update())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(moved.header.sequence, 2);
        assert!(moved.bytes.is_empty());
        assert_eq!(
            backend.video.produced.load(Ordering::SeqCst),
            frames as usize
        );
        h.legacy().await;
        viewer.close().await.unwrap();
        assert!(!cursor.is_current());
        assert_eq!(backend.active.load(Ordering::SeqCst), 0);
        assert_eq!(backend.video.active.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn screen_switch_invalidates_buffered_old_cursor_and_releases_before_reopen() {
    let backend = Arc::new(CursorBackend::default());
    let global = DesktopHost::new(Some(backend.clone()));
    let h = Harness::new(&global).await;
    let mut viewer = viewer(&h).await;
    viewer.next_frame(WAIT).await.unwrap();
    let mut old = viewer.open_cursor().await.unwrap(); // Leave initial image unread.
    viewer.select_screen("left".into()).await.unwrap();
    assert!(!old.is_current());
    assert!(matches!(old.next_update().await, Err(DesktopError::Closed)));
    assert_eq!(backend.active.load(Ordering::SeqCst), 0);
    let mut new = viewer.open_cursor().await.unwrap();
    let update = new.next_update().await.unwrap();
    assert_eq!(update.header.generation, 2);
    assert_eq!(update.header.sequence, 1);
    assert!(update.header.image.is_some());
    assert_eq!(update.header.position.unwrap().x, -959);
    assert_eq!(backend.maximum.load(Ordering::SeqCst), 1);
    viewer.close().await.unwrap();
}

#[tokio::test]
async fn cursor_reuses_connection_ownership_and_rejects_duplicate_or_bad_version() {
    let backend = Arc::new(CursorBackend::default());
    let global = DesktopHost::new(Some(backend.clone()));
    let h = Harness::new(&global).await;
    let other = Harness::new(&global).await;
    let mut viewer = viewer(&h).await;
    assert!(matches!(
        other.request(request(&viewer, 1)).await,
        RemoteDesktopReply::Error {
            code: DesktopErrorCode::WrongOwner,
            ..
        }
    ));
    assert!(matches!(
        h.request(request(&viewer, 2)).await,
        RemoteDesktopReply::Error {
            code: DesktopErrorCode::Unsupported,
            ..
        }
    ));
    assert_eq!(backend.active.load(Ordering::SeqCst), 0);
    let _cursor = viewer.open_cursor().await.unwrap();
    assert!(matches!(
        h.request(request(&viewer, 1)).await,
        RemoteDesktopReply::Error {
            code: DesktopErrorCode::StaleGeneration,
            ..
        }
    ));
    assert_eq!(backend.active.load(Ordering::SeqCst), 1);
    h.legacy().await;
    viewer.close().await.unwrap();
}

#[tokio::test]
async fn missing_or_unknown_cursor_capability_keeps_existing_video_usable() {
    let backend = Arc::new(Backend::default());
    let global = DesktopHost::new(Some(backend));
    assert_eq!(global.supported_cursor_version(), None);
    let h = Harness::new(&global).await;
    let mut viewer = RemoteDesktopSession::open(h.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    for version in [None, Some(999)] {
        viewer = viewer.with_cursor_support(version);
        assert!(!viewer.supports_cursor());
        assert!(matches!(
            viewer.open_cursor().await,
            Err(DesktopError::Rejected {
                code: DesktopErrorCode::Unsupported,
                ..
            })
        ));
    }
    assert!(matches!(
        h.request(request(&viewer, 1)).await,
        RemoteDesktopReply::Error {
            code: DesktopErrorCode::Unsupported,
            ..
        }
    ));
    viewer.next_frame(WAIT).await.unwrap();
    h.legacy().await;
    viewer.close().await.unwrap();
}

#[tokio::test]
async fn cursor_failure_preserves_video_input_and_shared_connection() {
    let backend = Arc::new(CursorBackend {
        fail: true,
        ..Default::default()
    });
    let global = DesktopHost::new(Some(backend.clone()));
    let h = Harness::new(&global).await;
    let mut viewer = viewer(&h).await;
    viewer.next_frame(WAIT).await.unwrap();
    let mut cursor = viewer.open_cursor().await.unwrap();
    cursor.next_update().await.unwrap();
    assert!(cursor.next_update().await.is_err());
    assert!(!cursor.is_current());
    eventually(|| backend.active.load(Ordering::SeqCst) == 0).await;
    assert_eq!(backend.video.active.load(Ordering::SeqCst), 1);
    viewer.set_input_enabled(true).await.unwrap();
    viewer
        .send_input(RemoteDesktopInput::Key {
            virtual_key: 17,
            down: true,
        })
        .await
        .unwrap();
    assert_eq!(backend.video.input_held.load(Ordering::SeqCst), 1);
    viewer.set_input_enabled(false).await.unwrap();
    assert_eq!(backend.video.input_held.load(Ordering::SeqCst), 0);
    viewer.next_frame(WAIT).await.unwrap();
    h.legacy().await;
    viewer.select_screen("primary".into()).await.unwrap();
    viewer
        .open_cursor()
        .await
        .unwrap()
        .next_update()
        .await
        .unwrap();
    viewer.close().await.unwrap();
}

#[tokio::test]
async fn dropping_cursor_releases_only_its_source_and_session_drop_cancels_retained_handle() {
    let backend = Arc::new(CursorBackend::default());
    let global = DesktopHost::new(Some(backend.clone()));
    let h = Harness::new(&global).await;
    let mut viewer = viewer(&h).await;
    let cursor = viewer.open_cursor().await.unwrap();
    drop(cursor);
    eventually(|| backend.active.load(Ordering::SeqCst) == 0).await;
    assert_eq!(backend.video.active.load(Ordering::SeqCst), 1);
    assert!(matches!(
        RemoteDesktopSession::open(h.client.clone(), None, 15, WAIT).await,
        Err(DesktopError::Rejected {
            code: DesktopErrorCode::Busy,
            ..
        })
    ));
    viewer.select_screen("primary".into()).await.unwrap();
    let mut retained = viewer.open_cursor().await.unwrap();
    drop(viewer);
    assert!(!retained.is_current());
    assert!(matches!(
        retained.next_update().await,
        Err(DesktopError::Closed)
    ));
    eventually(|| {
        backend.active.load(Ordering::SeqCst) == 0
            && backend.video.active.load(Ordering::SeqCst) == 0
    })
    .await;
    let mut replacement = RemoteDesktopSession::open(h.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    replacement.close().await.unwrap();
}

#[tokio::test]
async fn account_change_cancels_cursor_startup_and_leaves_no_occupied_source() {
    let backend = Arc::new(CursorBackend {
        pending_start: true,
        ..Default::default()
    });
    let global = DesktopHost::new(Some(backend.clone()));
    let h = Harness::new(&global).await;
    let viewer = viewer(&h).await;
    {
        let opening = viewer.open_cursor();
        tokio::pin!(opening);
        tokio::select! {
            _ = &mut opening => panic!("cursor startup should remain pending"),
            _ = eventually(|| backend.active.load(Ordering::SeqCst) == 1) => {},
        }
        h.host.recheck_account(Some(UserId::new()));
        assert!(tokio::time::timeout(WAIT, opening).await.unwrap().is_err());
    }
    eventually(|| {
        backend.active.load(Ordering::SeqCst) == 0
            && backend.video.active.load(Ordering::SeqCst) == 0
    })
    .await;
    // Opening future no longer borrows the session after its enclosing scope.
    drop(viewer);
    h.legacy().await;
}

#[tokio::test]
async fn cursor_credit_bounds_history_and_bad_ack_does_not_end_video() {
    let backend = Arc::new(CursorBackend::default());
    let global = DesktopHost::new(Some(backend.clone()));
    let h = Harness::new(&global).await;
    let mut viewer = viewer(&h).await;
    viewer.next_frame(WAIT).await.unwrap();
    let mut raw = h.client.open_bi(WAIT).await.unwrap();
    raw.send_frame_json(
        &DeviceTaskRequest::RemoteDesktop {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
            request: request(&viewer, DESKTOP_CURSOR_SCHEMA_VERSION),
        },
        WAIT,
    )
    .await
    .unwrap();
    assert!(matches!(
        raw.receive_json::<DeviceTaskResponse>(WAIT).await.unwrap(),
        DeviceTaskResponse::RemoteDesktop {
            reply: RemoteDesktopReply::CursorReady {
                version: DESKTOP_CURSOR_SCHEMA_VERSION
            }
        }
    ));
    let (mut outgoing, incoming) = raw.into_framed();
    let mut reader = DesktopCursorReader::new(incoming, viewer.opened().config.clone()).unwrap();
    reader.receive_after_idle(WAIT).await.unwrap(); // Deliberately no consumption ACK.
    backend.wake.notify_one();
    let next = reader.receive_after_idle(WAIT);
    tokio::pin!(next);
    tokio::select! {
        _ = &mut next => panic!("cursor history must stop at one outstanding update"),
        _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {},
    }
    assert_eq!(backend.produced.load(Ordering::SeqCst), 1);
    outgoing.write(&99_u64.to_le_bytes(), 8).await.unwrap();
    assert!(tokio::time::timeout(WAIT, next).await.unwrap().is_err());
    eventually(|| backend.active.load(Ordering::SeqCst) == 0).await;
    assert_eq!(backend.video.active.load(Ordering::SeqCst), 1);
    viewer.next_frame(WAIT).await.unwrap();
    h.legacy().await;
    viewer.close().await.unwrap();
}
