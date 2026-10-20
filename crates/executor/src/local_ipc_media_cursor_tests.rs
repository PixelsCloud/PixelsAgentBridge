use pab_protocol::{
    DESKTOP_CURSOR_SCHEMA_VERSION, DESKTOP_MAX_CURSOR_BYTES, DesktopCursorFormat,
    DesktopCursorHeader, DesktopCursorImage, DesktopCursorPosition,
};
use pab_remote_desktop::{CursorUpdate, DesktopCursorSource};

struct CursorBackend {
    video: Backend,
    active: Arc<AtomicUsize>,
    produced: Arc<AtomicUsize>,
    updates: Arc<tokio::sync::Semaphore>,
    pending_start: bool,
}
impl CursorBackend {
    fn new(static_video: bool, pending_start: bool) -> Self {
        Self {
            video: Backend {
                pending_frame: static_video,
                input_enabled: true,
                ..Default::default()
            },
            active: Arc::new(AtomicUsize::new(0)),
            produced: Arc::new(AtomicUsize::new(0)),
            updates: Arc::new(tokio::sync::Semaphore::new(1)),
            pending_start,
        }
    }
}
struct TestCursor {
    config: DesktopVideoConfig,
    active: Arc<AtomicUsize>,
    produced: Arc<AtomicUsize>,
    updates: Arc<tokio::sync::Semaphore>,
    sequence: u64,
}
impl Drop for TestCursor {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}
impl DesktopCursorSource for TestCursor {
    fn next_update(&mut self) -> MediaFuture<'_, CursorUpdate> {
        Box::pin(async {
            self.updates.acquire().await.unwrap().forget();
            self.sequence += 1;
            self.produced.fetch_add(1, Ordering::SeqCst);
            Ok(CursorUpdate {
                header: DesktopCursorHeader {
                    version: DESKTOP_CURSOR_SCHEMA_VERSION,
                    session_id: self.config.session_id,
                    generation: self.config.generation,
                    sequence: self.sequence,
                    position: (self.sequence % 2 == 1)
                        .then_some(DesktopCursorPosition { x: -1800, y: 25 }),
                    image_id: 1,
                    image: (self.sequence == 1).then_some(DesktopCursorImage {
                        format: DesktopCursorFormat::AndXorBgra,
                        width: 512,
                        height: 512,
                        hotspot_x: 511,
                        hotspot_y: 0,
                    }),
                },
                bytes: if self.sequence == 1 {
                    vec![0xf0; DESKTOP_MAX_CURSOR_BYTES]
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
    fn cursor(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopCursorSource>> {
        Box::pin(async move {
            self.active.fetch_add(1, Ordering::SeqCst);
            let cursor = TestCursor {
                config,
                active: self.active.clone(),
                produced: self.produced.clone(),
                updates: self.updates.clone(),
                sequence: 0,
            };
            if self.pending_start {
                std::future::pending::<()>().await;
            }
            Ok(Box::new(cursor) as Box<dyn DesktopCursorSource>)
        })
    }
    fn supports_input(&self) -> bool {
        true
    }
    fn input(
        &self,
        config: DesktopVideoConfig,
    ) -> MediaFuture<'_, Box<dyn pab_remote_desktop::DesktopInputSink>> {
        self.video.input(config)
    }
    fn screens(&self) -> MediaFuture<'_, Vec<DesktopScreen>> {
        self.video.screens()
    }
    fn start(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopSource>> {
        self.video.start(config)
    }
}
async fn cursor_pair(backend: Arc<dyn DesktopBackend>, provider_id: u64) -> Pair {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async_with_config(stream, None).await.unwrap();
        let frame: Value = read_control(&mut socket).await.unwrap();
        let _ = cursor::serve_registered(socket, frame).await;
    });
    let helper = tokio::spawn(async move {
        let (socket, _) = connect_async(format!("ws://{address}")).await.unwrap();
        let _ = cursor::serve_helper_socket(socket, backend.as_ref(), provider_id).await;
    });
    Pair { server, helper }
}
async fn cursor_ended(pair: &Pair, backend: &CursorBackend) {
    tokio::time::timeout(WAIT, async {
        while !pair.server.is_finished()
            || !pair.helper.is_finished()
            || backend.active.load(Ordering::SeqCst) != 0
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "requires an active local graphical user; run on the Linux VM with --ignored"
)]
async fn paired_cursor_binary_moves_during_static_video_without_blocking_input() {
    let _lock = LOCK.lock().await;
    let backend = Arc::new(CursorBackend::new(true, false));
    let capture_pair = pair(backend.clone()).await;
    let config = config();
    let mut video = HelperMediaBackend.start(config.clone()).await.unwrap();
    assert!(HelperMediaBackend.supports_cursor());
    let id = provider().await.unwrap().id;
    let cursor_pair = cursor_pair(backend.clone(), id).await;
    let input_pair = input_pair(backend.clone(), id).await;
    let mut input = HelperMediaBackend.input(config.clone()).await.unwrap();
    let mut cursor = HelperMediaBackend.cursor(config).await.unwrap();
    {
        let pending_video = video.next_frame();
        tokio::pin!(pending_video);
        tokio::select! {
            _ = &mut pending_video => panic!("static video unexpectedly returned"),
            _ = tokio::time::sleep(Duration::from_millis(50)) => {},
        }
        let first = tokio::time::timeout(WAIT, cursor.next_update())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.bytes, vec![0xf0; DESKTOP_MAX_CURSOR_BYTES]);
        assert_eq!(first.header.position.unwrap().x, -1800);
        assert_eq!(first.header.image.unwrap().hotspot_x, 511);
        backend.updates.add_permits(10);
        // Producing another snapshot requires another pull, regardless of how
        // many source changes arrived while the consumer was busy.
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(backend.produced.load(Ordering::SeqCst), 1);
        let hidden = cursor.next_update().await.unwrap();
        assert_eq!(hidden.header.sequence, 2);
        assert!(
            hidden.header.position.is_none()
                && hidden.header.image.is_none()
                && hidden.bytes.is_empty()
        );
        let moved = cursor.next_update().await.unwrap();
        assert_eq!(moved.header.sequence, 3);
        assert!(moved.header.position.is_some() && moved.bytes.is_empty());
        input
            .apply(pab_protocol::RemoteDesktopInput::Key {
                virtual_key: 65,
                down: true,
            })
            .await
            .unwrap();
        assert!(backend.video.held.load(Ordering::SeqCst));
        drop(cursor);
        cursor_ended(&cursor_pair, &backend).await;
        assert_eq!(backend.video.active.load(Ordering::SeqCst), 1);
        input
            .apply(pab_protocol::RemoteDesktopInput::ReleaseAll)
            .await
            .unwrap();
        assert!(!backend.video.held.load(Ordering::SeqCst));
    }
    drop(input);
    drop(input_pair);
    drop(video);
    ended(capture_pair, &backend.video).await;
    assert!(!HelperMediaBackend.supports_cursor());
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "requires an active local graphical user; run on the Linux VM with --ignored"
)]
async fn cursor_pairing_rejects_wrong_provider_duplicate_and_stale_layout_then_reopens() {
    let _lock = LOCK.lock().await;
    for generation in [1, 2] {
        let backend = Arc::new(CursorBackend::new(false, false));
        let capture_pair = pair(backend.clone()).await;
        let mut config = config();
        config.generation = generation;
        let mut video = HelperMediaBackend.start(config.clone()).await.unwrap();
        let capture = provider().await.unwrap();
        let wrong = cursor_pair(backend.clone(), u64::MAX).await;
        cursor_ended(&wrong, &backend).await;
        assert!(capture.cursor.lock().unwrap().is_none());
        let paired = cursor_pair(backend.clone(), capture.id).await;
        tokio::time::timeout(WAIT, async {
            while capture.cursor.lock().unwrap().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let duplicate = cursor_pair(backend.clone(), capture.id).await;
        cursor_ended(&duplicate, &backend).await;
        for stale_layout in [false, true] {
            let mut stale = config.clone();
            if stale_layout {
                stale.screen.x += 100;
            } else {
                stale.generation += 1;
            }
            assert!(HelperMediaBackend.cursor(stale).await.is_err());
        }
        let mut cursor = HelperMediaBackend.cursor(config.clone()).await.unwrap();
        let update = cursor.next_update().await.unwrap();
        assert_eq!(update.header.generation, generation);
        assert_eq!(update.header.sequence, 1);
        assert_eq!(video.next_frame().await.unwrap().bytes.len(), 200_013);
        drop(video);
        cursor_ended(&paired, &backend).await;
        assert!(cursor.next_update().await.is_err());
        drop(cursor);
        ended(capture_pair, &backend.video).await;
    }
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "requires an active local graphical user; run on the Linux VM with --ignored"
)]
async fn cancelling_cursor_start_or_idle_read_releases_only_cursor() {
    let _lock = LOCK.lock().await;
    for pending_start in [true, false] {
        let backend = Arc::new(CursorBackend::new(false, pending_start));
        let capture_pair = pair(backend.clone()).await;
        let config = config();
        let mut video = HelperMediaBackend.start(config.clone()).await.unwrap();
        let paired = cursor_pair(backend.clone(), provider().await.unwrap().id).await;
        if pending_start {
            // Wait until the native source really started before cancelling,
            // avoiding a test which merely times out during peer registration.
            let opening = HelperMediaBackend.cursor(config);
            tokio::pin!(opening);
            tokio::select! {
                _ = &mut opening => panic!("cursor startup unexpectedly returned"),
                _ = tokio::time::timeout(WAIT, async {
                    while backend.active.load(Ordering::SeqCst) == 0 { tokio::task::yield_now().await; }
                }) => {},
            }
            assert_eq!(backend.active.load(Ordering::SeqCst), 1);
        } else {
            let mut cursor = HelperMediaBackend.cursor(config).await.unwrap();
            cursor.next_update().await.unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(100), cursor.next_update())
                    .await
                    .is_err()
            );
            cursor_ended(&paired, &backend).await;
            assert!(cursor.next_update().await.is_err());
        }
        cursor_ended(&paired, &backend).await;
        assert_eq!(video.next_frame().await.unwrap().bytes.len(), 200_013);
        drop(video);
        ended(capture_pair, &backend.video).await;
    }
}

// Send invalid native-helper replies directly: helper-side validation must not
// conceal bugs in the Executor's own validation/allocation boundary.
async fn malformed_cursor_pair(provider_id: u64, case: usize) -> Pair {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async_with_config(stream, None).await.unwrap();
        let frame: Value = read_control(&mut socket).await.unwrap();
        let _ = cursor::serve_registered(socket, frame).await;
    });
    let helper = tokio::spawn(async move {
        let (mut socket, _) = connect_async(format!("ws://{address}")).await.unwrap();
        send_json(
            &mut socket,
            &json!({"type":"register_media_cursor_helper", "version":DESKTOP_CURSOR_SCHEMA_VERSION,
            "provider_id":provider_id, "process_id":std::process::id()}),
        )
        .await
        .unwrap();
        loop {
            let frame: Value = read_control(&mut socket).await.unwrap();
            match frame["type"].as_str() {
                Some("verify_helper_identity") => helper_identity::answer(&frame).await.unwrap(),
                Some("media_cursor_registered") => break,
                _ => panic!("unexpected cursor handshake"),
            }
        }
        let operation: Value = read_control(&mut socket).await.unwrap();
        assert_eq!(operation["operation"], "start");
        let config: DesktopVideoConfig =
            serde_json::from_value(operation["config"].clone()).unwrap();
        send_json(&mut socket, &json!({"result":"started"}))
            .await
            .unwrap();
        let operation: Value = read_control(&mut socket).await.unwrap();
        assert_eq!(operation["operation"], "next");
        let mut header = DesktopCursorHeader {
            version: DESKTOP_CURSOR_SCHEMA_VERSION,
            session_id: config.session_id,
            generation: config.generation,
            sequence: 1,
            position: Some(DesktopCursorPosition { x: -1900, y: 10 }),
            image_id: 1,
            image: Some(DesktopCursorImage {
                format: DesktopCursorFormat::PremultipliedBgra,
                width: 1,
                height: 1,
                hotspot_x: 0,
                hotspot_y: 0,
            }),
        };
        match case {
            0 => header.image.as_mut().unwrap().width = u32::MAX,
            1 => header.generation += 1,
            2 => header.sequence = 2,
            3 => {
                header.image = None;
                header.image_id = 99;
            }
            4 | 5 => {}
            _ => unreachable!(),
        }
        send_json(&mut socket, &json!({"result":"update", "header":header}))
            .await
            .unwrap();
        if case == 4 || case == 5 {
            // 4: truncated but still-open source; 5: chunk exceeds remaining.
            socket
                .send(Message::Binary(
                    vec![0; if case == 4 { 2 } else { 5 }].into(),
                ))
                .await
                .unwrap();
        }
        // Invalid headers must be rejected without waiting for image bytes.
        let _ = receive_frame(&mut socket).await;
    });
    Pair { server, helper }
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "requires an active local graphical user; run on the Linux VM with --ignored"
)]
async fn malformed_cursor_header_and_partial_payload_retire_only_cursor() {
    let _lock = LOCK.lock().await;
    for case in 0..6 {
        let backend = Arc::new(CursorBackend::new(false, false));
        let capture_pair = pair(backend.clone()).await;
        let config = config();
        let mut video = HelperMediaBackend.start(config.clone()).await.unwrap();
        let mut paired = malformed_cursor_pair(provider().await.unwrap().id, case).await;
        let mut cursor = HelperMediaBackend.cursor(config).await.unwrap();
        let wait = if case == 4 {
            AUTH_TIMEOUT + Duration::from_secs(2)
        } else {
            Duration::from_secs(2)
        };
        assert!(
            tokio::time::timeout(wait, cursor.next_update())
                .await
                .unwrap()
                .is_err(),
            "case {case}"
        );
        cursor_ended(&paired, &backend).await;
        // Inspect join results: a panicked fake helper is not rejection evidence.
        assert!((&mut paired.server).await.is_ok());
        assert!((&mut paired.helper).await.is_ok());
        assert_eq!(video.next_frame().await.unwrap().bytes.len(), 200_013);
        assert!(cursor.next_update().await.is_err());
        drop(cursor);
        drop(video);
        ended(capture_pair, &backend.video).await;
    }
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "requires an active local graphical user; run on the Linux VM with --ignored"
)]
async fn cursor_capability_tracks_capture_provider_and_preserves_legacy_helper() {
    let _lock = LOCK.lock().await;
    let backend = Arc::new(Backend::default());
    let legacy = pair(backend.clone()).await;
    let config = config();
    let mut video = HelperMediaBackend.start(config.clone()).await.unwrap();
    assert!(!HelperMediaBackend.supports_cursor());
    assert!(HelperMediaBackend.cursor(config).await.is_err());
    let wrong = cursor_pair(backend.clone(), provider().await.unwrap().id).await;
    tokio::time::timeout(WAIT, async {
        while !wrong.server.is_finished() || !wrong.helper.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(provider().await.unwrap().cursor.lock().unwrap().is_none());
    assert_eq!(video.next_frame().await.unwrap().bytes.len(), 200_013);
    drop(video);
    ended(legacy, &backend).await;
    assert!(!HelperMediaBackend.supports_cursor());

    let capable = Arc::new(CursorBackend::new(false, false));
    let mut active_pair = pair(capable.clone()).await;
    let active = HelperMediaBackend.start(super::config()).await.unwrap();
    assert!(HelperMediaBackend.supports_cursor());
    let later_legacy = pair(backend.clone()).await;
    tokio::time::timeout(WAIT, async {
        while providers().lock().unwrap().len() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // The newest registration is not the capture already serving this viewer.
    assert!(HelperMediaBackend.supports_cursor());
    drop(active);
    tokio::time::timeout(WAIT, async {
        assert!((&mut active_pair.server).await.is_ok());
        assert!((&mut active_pair.helper).await.is_ok());
    })
    .await
    .unwrap();
    assert_eq!(capable.video.active.load(Ordering::SeqCst), 0);
    assert!(!HelperMediaBackend.supports_cursor());
    let mut fallback = HelperMediaBackend.start(super::config()).await.unwrap();
    assert_eq!(fallback.next_frame().await.unwrap().bytes.len(), 200_013);
    drop(fallback);
    ended(later_legacy, &backend).await;
}
