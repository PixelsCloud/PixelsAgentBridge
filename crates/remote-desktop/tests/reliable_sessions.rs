//! Actual Iroh/QUIC streams on loopback. The actor represents the existing
//! authentication result; these tests do not claim to test password auth or
//! native H.264. Payloads are deterministic transport fixtures, not real video.
use pab_protocol::*;
use pab_remote_desktop::*;
use pab_transport::*;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicUsize, Ordering},
    },
    time::Duration,
};
const WAIT: Duration = Duration::from_secs(5);
mod cursor_sessions;
mod cursor_wire;
mod flow_recovery;
mod clipboard_sessions;
mod input_wire;
mod quality_feedback;
mod input_progress;
#[path = "../../../smoke/remote_desktop_network_recovery.rs"]
mod network_recovery;

fn screens() -> Vec<DesktopScreen> {
    vec![
        DesktopScreen {
            id: "primary".into(),
            name: "Main".into(),
            x: 0,
            y: 0,
            desktop_width: 1920,
            desktop_height: 1080,
            pixel_width: 1920,
            pixel_height: 1080,
            scale_milli: 1000,
            rotation: DesktopRotation::Identity,
            primary: true,
        },
        DesktopScreen {
            id: "left".into(),
            name: "Left".into(),
            x: -960,
            y: -100,
            desktop_width: 960,
            desktop_height: 540,
            pixel_width: 1920,
            pixel_height: 1080,
            scale_milli: 2000,
            rotation: DesktopRotation::Identity,
            primary: false,
        },
    ]
}
fn config() -> DesktopVideoConfig {
    DesktopVideoConfig {
        session_id: RequestId::new(),
        generation: 1,
        screen: screens().remove(0),
        codec: DesktopCodec::H264AnnexB,
        color: DesktopColor::Bt709Limited,
        fps: 15,
    }
}
#[derive(Default)]
struct Backend {
    budget: Option<Arc<AtomicU32>>,
    frame_bytes: Option<usize>,
    active: Arc<AtomicUsize>,
    produced: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
    input_active: Arc<AtomicUsize>,
    input_held: Arc<AtomicUsize>,
    input_applied: Arc<AtomicUsize>,
    fail_input: Option<Arc<AtomicUsize>>,
    input_block: Option<Arc<tokio::sync::Notify>>,
    wake: Option<Arc<tokio::sync::Notify>>,
    fail_after: Option<u64>,
    encode_delay: Option<Duration>,
}
struct InputSink {
    active: Arc<AtomicUsize>,
    held: Arc<AtomicUsize>,
    applied: Arc<AtomicUsize>,
    wake: Option<Arc<tokio::sync::Notify>>,
    block: Option<Arc<tokio::sync::Notify>>,
}
impl Drop for InputSink {
    fn drop(&mut self) {
        self.held.store(0, Ordering::SeqCst);
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}
impl DesktopInputSink for InputSink {
    fn apply(&mut self, event: RemoteDesktopInput) -> MediaFuture<'_, ()> {
        Box::pin(async move {
            self.applied.fetch_add(1, Ordering::SeqCst);
            match event {
                RemoteDesktopInput::Key { down, .. } => {
                    self.held.store(usize::from(down), Ordering::SeqCst)
                }
                RemoteDesktopInput::ReleaseAll => self.held.store(0, Ordering::SeqCst),
                _ => {}
            }
            if let Some(block) = &self.block { block.notified().await; }
            if let Some(wake) = &self.wake {
                wake.notify_one();
            }
            Ok(())
        })
    }
}
struct Source {
    budget: Option<Arc<AtomicU32>>,
    frame_bytes: usize,
    backend: Arc<AtomicUsize>,
    produced: Arc<AtomicUsize>,
    number: u64,
    marker: u8,
    wake: Option<Arc<tokio::sync::Notify>>,
    fail_after: Option<u64>,
    encode_delay: Option<Duration>,
}
impl Drop for Source {
    fn drop(&mut self) {
        self.backend.fetch_sub(1, Ordering::SeqCst);
    }
}
impl DesktopSource for Source {
    fn encoding_budget(&self) -> Option<u32> {
        self.budget.as_ref().map(|budget| budget.load(Ordering::SeqCst))
    }
    fn set_encoding_budget(&mut self, value: u32) -> MediaFuture<'_, ()> {
        Box::pin(async move {
            self.budget.as_ref().ok_or(DesktopError::Closed)?.store(value, Ordering::SeqCst);
            Ok(())
        })
    }
    fn next_frame(&mut self) -> MediaFuture<'_, MediaFrame> {
        Box::pin(async move {
            tokio::task::yield_now().await;
            if self.number > 0 {
                if let Some(wake) = &self.wake {
                    wake.notified().await;
                }
            }
            if self.fail_after.is_some_and(|count| self.number >= count) {
                return Err(DesktopError::Closed);
            }
            if let Some(delay) = self.encode_delay {
                tokio::time::sleep(delay).await;
            }
            self.number += 1;
            self.produced.fetch_add(1, Ordering::SeqCst);
            Ok(MediaFrame {
                bytes: vec![self.marker; self.frame_bytes],
                capture_time_us: self.number * 66667,
                keyframe: self.number == 1,
            })
        })
    }
}
impl DesktopBackend for Backend {
    fn supports_input(&self) -> bool {
        true
    }
    fn input(&self, _: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopInputSink>> {
        Box::pin(async move {
            match self.fail_input.as_ref().map(|fail| fail.swap(0, Ordering::SeqCst)) {
                Some(1) => return Err(DesktopError::Closed),
                Some(2) => return Err(DesktopError::Timeout),
                _ => {}
            }
            self.input_active.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(InputSink {
                active: self.input_active.clone(),
                held: self.input_held.clone(),
                applied: self.input_applied.clone(),
                wake: self.wake.clone(),
                block: self.input_block.clone(),
            }) as Box<dyn DesktopInputSink>)
        })
    }
    fn screens(&self) -> MediaFuture<'_, Vec<DesktopScreen>> {
        Box::pin(async { Ok(screens()) })
    }
    fn start(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopSource>> {
        Box::pin(async move {
            let count = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(count, Ordering::SeqCst);
            Ok(Box::new(Source {
                budget: self.budget.clone(),
                frame_bytes: self.frame_bytes.unwrap_or(512 * 1024 + 13),
                backend: self.active.clone(),
                produced: self.produced.clone(),
                number: 0,
                wake: self.wake.clone(),
                fail_after: self.fail_after,
                encode_delay: self.encode_delay,
                marker: if config.screen.id == "left" {
                    0x55
                } else {
                    0xaa
                },
            }) as Box<dyn DesktopSource>)
        })
    }
}
fn actor() -> OperatorRef {
    OperatorRef::guest(EndpointKey::new([7; 32]))
}
async fn pair() -> (PabEndpoint, PabEndpoint, PabConnection, PabConnection) {
    let config = PabEndpointConfig::new(vec!["https://127.0.0.1:1".parse().unwrap()]).unwrap();
    let a = PabEndpoint::bind(config.clone(), iroh_base::SecretKey::generate())
        .await
        .unwrap();
    let b = PabEndpoint::bind(config, iroh_base::SecretKey::generate())
        .await
        .unwrap();
    let mut addresses = b.watch_address();
    tokio::time::timeout(WAIT, async {
        while addresses.borrow().direct_addresses.is_empty() {
            addresses.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let direct_addresses = addresses
        .borrow()
        .direct_addresses
        .iter()
        .map(|a| {
            std::net::SocketAddr::new(
                if a.is_ipv4() {
                    std::net::Ipv4Addr::LOCALHOST.into()
                } else {
                    std::net::Ipv6Addr::LOCALHOST.into()
                },
                a.port(),
            )
        })
        .collect();
    let remote = b.clone();
    let accept = tokio::spawn(async move { remote.accept().await.unwrap().unwrap() });
    let client = a
        .connect(
            *b.id().as_bytes(),
            &PabEndpointAddress {
                direct_addresses,
                relay_urls: vec![],
            },
            WAIT,
        )
        .await
        .unwrap();
    (a, b, client, accept.await.unwrap())
}
struct Harness {
    _a: PabEndpoint,
    _b: PabEndpoint,
    client: PabConnection,
    peer: PabConnection,
    task: tokio::task::JoinHandle<()>,
    host: DesktopHost,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
        self.client.close(b"isolated test ended");
    }
}
impl Harness {
    async fn new(global: &DesktopHost) -> Self {
        Self::with_pair(global, pair().await).await
    }
    async fn with_pair(global: &DesktopHost, endpoints: (PabEndpoint, PabEndpoint, PabConnection, PabConnection)) -> Self {
        Self::with_account(global,endpoints,None).await
    }
    async fn with_account(global:&DesktopHost,endpoints:(PabEndpoint,PabEndpoint,PabConnection,PabConnection),account:Option<UserId>)->Self {
        let (a, b, client, peer) = endpoints;
        let server = peer.clone();
        let host = global.for_connection();
        let authority = host.clone();
        let task = tokio::spawn(async move {
            let _guard = host.connection_guard();
            loop {
                let mut stream = match server.accept_bi(WAIT).await {
                    Ok(s) => s,
                    Err(PabConnectionError::Timeout) => continue,
                    Err(_) => break,
                };
                let host = host.clone();
                tokio::spawn(async move {
                    match stream
                        .receive_json::<DeviceTaskRequest>(WAIT)
                        .await
                        .unwrap()
                    {
                        DeviceTaskRequest::RemoteDesktop {
                            request,
                            schema_version: DEVICE_TASK_SCHEMA_VERSION,
                        } => {
                            let _ = host.serve(actor(), account, request, stream, WAIT).await;
                        }
                        DeviceTaskRequest::GetPresence { .. } => {
                            stream
                                .send_json(&serde_json::json!({"legacy_probe":"ok"}), WAIT)
                                .await
                                .unwrap();
                        }
                        other => panic!("unexpected request {other:?}"),
                    }
                });
            }
        });
        Self {
            _a: a,
            _b: b,
            client,
            peer,
            task,
            host: authority,
        }
    }
    async fn request(&self, request: RemoteDesktopRequest) -> RemoteDesktopReply {
        let mut stream = self.client.open_bi(WAIT).await.unwrap();
        stream
            .send_frame_json(
                &DeviceTaskRequest::RemoteDesktop {
                    schema_version: DEVICE_TASK_SCHEMA_VERSION,
                    request,
                },
                WAIT,
            )
            .await
            .unwrap();
        match stream.receive_json(WAIT).await.unwrap() {
            DeviceTaskResponse::RemoteDesktop { reply } => reply,
            other => panic!("{other:?}"),
        }
    }
    async fn legacy(&self) {
        let mut stream = self.client.open_bi(WAIT).await.unwrap();
        stream
            .send_frame_json(
                &DeviceTaskRequest::GetPresence {
                    schema_version: DEVICE_TASK_SCHEMA_VERSION,
                },
                WAIT,
            )
            .await
            .unwrap();
        assert_eq!(
            stream
                .receive_json::<serde_json::Value>(WAIT)
                .await
                .unwrap(),
            serde_json::json!({"legacy_probe":"ok"})
        );
    }
}
async fn eventually(condition: impl Fn() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn large_frames_credit_screen_switch_and_legacy_stream_coexist() {
    let backend = Arc::new(Backend::default());
    let global = DesktopHost::new(Some(backend.clone()));
    let server = Harness::new(&global).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    let session_id = viewer.opened().config.session_id;
    let old_presentations = viewer.presentation_control();
    let mut old_header = None;
    for index in 1..=DESKTOP_MAX_IN_FLIGHT {
        let frame = viewer.next_frame(WAIT).await.unwrap();
        assert_eq!(frame.header.frame_id, index);
        assert_eq!(frame.bytes, vec![0xaa; 512 * 1024 + 13]);
        old_header = Some(frame.header);
    }
    let old_header = old_header.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        backend.produced.load(Ordering::SeqCst),
        DESKTOP_MAX_IN_FLIGHT as usize
    );
    server.legacy().await; // Saturated video credit must not block another stream.
    viewer.report_presented(&old_header).unwrap();
    let fifth = viewer.next_frame(WAIT).await.unwrap();
    assert_eq!(fifth.header.frame_id, 5);
    viewer.select_screen("left".into()).await.unwrap();
    assert!(old_presentations.report_presented(&old_header).is_err());
    assert_eq!(viewer.opened().config.generation, 2);
    assert_eq!(viewer.opened().config.screen.x, -960);
    assert_eq!(viewer.opened().config.screen.scale_milli, 2000);
    let switched = viewer.next_frame(WAIT).await.unwrap();
    assert_eq!(switched.header.frame_id, 1);
    assert!(switched.header.keyframe);
    assert_eq!(switched.bytes, vec![0x55; 512 * 1024 + 13]);
    assert!(viewer.report_presented(&old_header).is_err());
    assert!(
        old_presentations
            .report_presented(&switched.header)
            .is_err()
    );
    let current_presentations = viewer.presentation_control();
    // Decoder output may arrive independently while the next network frame is
    // pending; this must neither cancel the read nor grant stale generation credit.
    let mut last = switched.header.clone();
    for _ in 1..DESKTOP_MAX_IN_FLIGHT {
        last = viewer.next_frame(WAIT).await.unwrap().header;
    }
    {
        let reading = viewer.next_frame(WAIT);
        tokio::pin!(reading);
        // Keep the same partially pending read alive while decoder output
        // independently returns credit. Cancelling and restarting it is unsafe.
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut reading)
                .await
                .is_err()
        );
        current_presentations.report_presented(&last).unwrap();
        assert_eq!(reading.await.unwrap().header.frame_id, 5);
    }
    assert!(matches!(
        server
            .request(RemoteDesktopRequest::Video {
                session_id,
                generation: 1
            })
            .await,
        RemoteDesktopReply::Error {
            code: DesktopErrorCode::StaleGeneration,
            ..
        }
    ));
    assert_eq!(backend.maximum.load(Ordering::SeqCst), 1);
    viewer.close().await.unwrap();
    assert!(
        current_presentations
            .report_presented(&switched.header)
            .is_err()
    );
    viewer.close().await.unwrap();
    eventually(|| backend.active.load(Ordering::SeqCst) == 0).await;
    server.legacy().await;
}

#[tokio::test]
async fn only_one_viewer_even_same_account_and_old_close_cannot_remove_new_owner() {
    let backend = Arc::new(Backend::default());
    let global = DesktopHost::new(Some(backend.clone()));
    let first = Harness::new(&global).await;
    let second = Harness::new(&global).await; // same actor, different connection
    let mut viewer = RemoteDesktopSession::open(first.client.clone(), None, 30, WAIT)
        .await
        .unwrap();
    let old_id = viewer.opened().config.session_id;
    assert!(matches!(
        RemoteDesktopSession::open(second.client.clone(), None, 15, WAIT).await,
        Err(DesktopError::Rejected {
            code: DesktopErrorCode::Busy,
            ..
        })
    ));
    assert!(matches!(
        second
            .request(RemoteDesktopRequest::Close { session_id: old_id })
            .await,
        RemoteDesktopReply::Error {
            code: DesktopErrorCode::WrongOwner,
            ..
        }
    ));
    assert!(matches!(
        second
            .request(RemoteDesktopRequest::Video {
                session_id: old_id,
                generation: 1
            })
            .await,
        RemoteDesktopReply::Error {
            code: DesktopErrorCode::WrongOwner,
            ..
        }
    ));
    viewer.next_frame(WAIT).await.unwrap();
    viewer.close().await.unwrap();
    eventually(|| backend.active.load(Ordering::SeqCst) == 0).await;
    let mut next = RemoteDesktopSession::open(second.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    assert_eq!(
        first
            .request(RemoteDesktopRequest::Close { session_id: old_id })
            .await,
        RemoteDesktopReply::Closed
    );
    next.next_frame(WAIT).await.unwrap();
    next.close().await.unwrap();
    second.legacy().await;
}

#[tokio::test]
async fn connection_loss_releases_capture_and_allows_another_viewer() {
    let backend = Arc::new(Backend::default());
    let global = DesktopHost::new(Some(backend.clone()));
    let first = Harness::new(&global).await;
    let second = Harness::new(&global).await;
    let mut viewer = RemoteDesktopSession::open(first.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    viewer.next_frame(WAIT).await.unwrap();
    first.peer.close(b"test connection loss");
    eventually(|| backend.active.load(Ordering::SeqCst) == 0).await;
    let mut replacement = RemoteDesktopSession::open(second.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    drop(viewer); // asynchronous old cleanup may race; it must not release replacement
    replacement.next_frame(WAIT).await.unwrap();
    second.legacy().await;
    replacement.close().await.unwrap();
}

#[tokio::test]
async fn partial_frame_timeout_and_cancel_poison_only_that_video_stream() {
    let (_a, _b, client, peer) = pair().await;
    for cancel in [false, true] {
        let config = config();
        let header = DesktopFrameHeader {
            version: REMOTE_DESKTOP_SCHEMA_VERSION,
            session_id: config.session_id,
            generation: 1,
            frame_id: 1,
            capture_time_us: 0,
            keyframe: true,
            payload_bytes: 100,
        };
        let mut sent = client.open_bi(WAIT).await.unwrap();
        sent.send_frame_json(&header, WAIT).await.unwrap();
        sent.send_binary_frame(&[7; 10], WAIT).await.unwrap(); // incomplete payload, sender stays alive
        let (_, reader) = peer.accept_bi(WAIT).await.unwrap().into_framed();
        let mut video = DesktopVideoReader::new(reader, config).unwrap();
        if cancel {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(30)) => {},
                _ = video.receive(WAIT) => panic!("partial frame should still be pending"),
            }
        } else {
            assert!(matches!(
                video.receive(Duration::from_millis(30)).await,
                Err(DesktopError::Timeout)
            ));
        }
        assert!(matches!(
            video.receive(WAIT).await,
            Err(DesktopError::Closed)
        ));
        let mut other = client.open_bi(WAIT).await.unwrap();
        other.send_frame_json(&42, WAIT).await.unwrap();
        assert_eq!(
            peer.accept_bi(WAIT)
                .await
                .unwrap()
                .receive_json::<u32>(WAIT)
                .await
                .unwrap(),
            42
        );
    }
}

#[tokio::test]
async fn idle_video_waits_for_changes_but_bounds_partial_payload() {
    let (_a, _b, client, peer) = pair().await;
    let config = config();
    let header = DesktopFrameHeader {
        version: REMOTE_DESKTOP_SCHEMA_VERSION,
        session_id: config.session_id,
        generation: 1,
        frame_id: 1,
        capture_time_us: 0,
        keyframe: true,
        payload_bytes: 100,
    };
    let mut sent = client.open_bi(WAIT).await.unwrap();
    // Announce the stream, then leave it completely idle for longer than the
    // per-packet timeout. The first metadata frame is test setup, not video.
    sent.send_frame_json(&42, WAIT).await.unwrap();
    let mut received = peer.accept_bi(WAIT).await.unwrap();
    assert_eq!(received.receive_json::<u32>(WAIT).await.unwrap(), 42);
    let (_, reader) = received.into_framed();
    let mut video = DesktopVideoReader::new(reader, config).unwrap();
    let waiting = tokio::spawn(async move {
        let result = video.receive_after_idle(Duration::from_millis(150)).await;
        (video, result)
    });
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(
        !waiting.is_finished(),
        "static desktop must remain connected"
    );
    sent.send_frame_json(&header, WAIT).await.unwrap();
    sent.send_binary_frame(&[7; 10], WAIT).await.unwrap();
    let (mut video, result) = tokio::time::timeout(WAIT, waiting).await.unwrap().unwrap();
    assert!(matches!(result, Err(DesktopError::Timeout)));
    assert!(matches!(
        video.receive_after_idle(WAIT).await,
        Err(DesktopError::Closed)
    ));
}

#[tokio::test]
async fn oversized_stale_or_non_key_first_frame_is_rejected_before_payload() {
    let (_a, _b, client, peer) = pair().await;
    for case in 0..4 {
        let config = config();
        let mut header = DesktopFrameHeader {
            version: REMOTE_DESKTOP_SCHEMA_VERSION,
            session_id: config.session_id,
            generation: 1,
            frame_id: 1,
            capture_time_us: 0,
            keyframe: true,
            payload_bytes: 100,
        };
        match case {
            0 => header.payload_bytes = DESKTOP_MAX_FRAME_BYTES as u32 + 1,
            1 => header.generation = 2,
            2 => header.keyframe = false,
            _ => header.frame_id = 2,
        }
        let mut sent = client.open_bi(WAIT).await.unwrap();
        sent.send_frame_json(&header, WAIT).await.unwrap();
        let (_, reader) = peer.accept_bi(WAIT).await.unwrap().into_framed();
        let mut video = DesktopVideoReader::new(reader, config).unwrap();
        assert!(matches!(
            video.receive(WAIT).await,
            Err(DesktopError::Protocol(_))
        ));
        assert!(matches!(
            video.receive(WAIT).await,
            Err(DesktopError::Closed)
        ));
    }
}

#[tokio::test]
async fn no_backend_means_no_capability_and_open_does_not_reserve_device() {
    let host = DesktopHost::default();
    assert_eq!(host.supported_version(), None);
    let server = Harness::new(&host).await;
    for _ in 0..2 {
        assert!(matches!(
            RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT).await,
            Err(DesktopError::Rejected {
                code: DesktopErrorCode::Unsupported,
                ..
            })
        ));
    }
    server.legacy().await;
}

#[tokio::test]
async fn input_is_opt_in_and_released_on_disable_screen_change_and_disconnect() {
    let backend = Arc::new(Backend::default());
    let host = DesktopHost::new(Some(backend.clone()));
    let server = Harness::new(&host).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    viewer.next_frame(WAIT).await.unwrap();
    let press = RemoteDesktopInput::Key {
        virtual_key: 65,
        down: true,
    };
    assert!(viewer.send_input(press.clone()).await.is_err());
    assert_eq!(backend.input_active.load(Ordering::SeqCst), 0);
    for finish in ["release", "disable", "switch", "disconnect"] {
        viewer.set_input_enabled(true).await.unwrap();
        viewer.send_input(press.clone()).await.unwrap();
        assert_eq!(backend.input_held.load(Ordering::SeqCst), 1);
        match finish {
            "release" => {
                viewer
                    .send_input(RemoteDesktopInput::ReleaseAll)
                    .await
                    .unwrap();
                assert_eq!(backend.input_held.load(Ordering::SeqCst), 0);
                viewer.set_input_enabled(false).await.unwrap();
            }
            "disable" => viewer.set_input_enabled(false).await.unwrap(),
            "switch" => {
                let obsolete = viewer.input_control();
                viewer.select_screen("left".into()).await.unwrap();
                viewer.next_frame(WAIT).await.unwrap();
                assert!(viewer.send_input(press.clone()).await.is_err());
                viewer.set_input_enabled(true).await.unwrap();
                assert!(matches!(
                    obsolete.set_enabled(false).await,
                    Err(DesktopError::Rejected {
                        code: DesktopErrorCode::StaleGeneration,
                        ..
                    })
                ));
                assert!(matches!(
                    obsolete.send(press.clone()).await,
                    Err(DesktopError::Rejected {
                        code: DesktopErrorCode::StaleGeneration,
                        ..
                    })
                ));
                assert_eq!(backend.input_active.load(Ordering::SeqCst), 1);
                viewer.set_input_enabled(false).await.unwrap();
            }
            _ => server.peer.close(b"test input connection loss"),
        }
        eventually(|| backend.input_active.load(Ordering::SeqCst) == 0).await;
        assert_eq!(backend.input_held.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn input_wakes_static_capture_without_cancelling_pending_video_read() {
    let backend = Arc::new(Backend {
        wake: Some(Arc::new(tokio::sync::Notify::new())),
        ..Default::default()
    });
    let host = DesktopHost::new(Some(backend));
    let server = Harness::new(&host).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    let first = viewer.next_frame(WAIT).await.unwrap();
    viewer.report_presented(&first.header).unwrap();
    let input = viewer.input_control();
    input.set_enabled(true).await.unwrap();
    let receiving = viewer.next_frame_when_changed();
    tokio::pin!(receiving);
    tokio::select! {
        _ = &mut receiving => panic!("static capture must wait for input"),
        _ = tokio::time::sleep(Duration::from_millis(50)) => {},
    }
    input
        .send(RemoteDesktopInput::Key {
            virtual_key: 65,
            down: true,
        })
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(WAIT, receiving)
            .await
            .unwrap()
            .unwrap()
            .header
            .frame_id,
        2
    );
    input.set_enabled(false).await.unwrap();
    server.legacy().await;
}

#[tokio::test]
async fn media_failure_releases_input_without_closing_shared_connection() {
    let backend = Arc::new(Backend {
        wake: Some(Arc::new(tokio::sync::Notify::new())),
        fail_after: Some(1),
        ..Default::default()
    });
    let host = DesktopHost::new(Some(backend.clone()));
    let server = Harness::new(&host).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    viewer.next_frame(WAIT).await.unwrap();
    viewer.set_input_enabled(true).await.unwrap();
    viewer
        .send_input(RemoteDesktopInput::Key {
            virtual_key: 65,
            down: true,
        })
        .await
        .unwrap();
    assert!(viewer.next_frame(WAIT).await.is_err());
    eventually(|| backend.input_active.load(Ordering::SeqCst) == 0).await;
    assert_eq!(backend.input_held.load(Ordering::SeqCst), 0);
    server.legacy().await;
    viewer.select_screen("primary".into()).await.unwrap();
    viewer.next_frame(WAIT).await.unwrap();
    viewer.set_input_enabled(true).await.unwrap();
    viewer.close().await.unwrap();
    eventually(|| backend.input_active.load(Ordering::SeqCst) == 0).await;
}

#[tokio::test]
async fn reopening_after_media_failure_does_not_replay_input_or_retain_ownership() {
    let backend = Arc::new(Backend {
        wake: Some(Arc::new(tokio::sync::Notify::new())),
        fail_after: Some(1),
        ..Default::default()
    });
    let host = DesktopHost::new(Some(backend.clone()));
    let server = Harness::new(&host).await;
    let mut old = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    old.next_frame(WAIT).await.unwrap();
    old.set_input_enabled(true).await.unwrap();
    let previous_control = old.input_control();
    let press = RemoteDesktopInput::Key {
        virtual_key: 0xa2,
        down: true,
    };
    old.send_input(press.clone()).await.unwrap();
    let failure = match old.next_frame(WAIT).await {
        Err(error) => error,
        Ok(_) => panic!("capture should fail after the first frame"),
    };
    assert!(failure.is_recoverable_media());
    old.close().await.unwrap();
    eventually(|| backend.input_active.load(Ordering::SeqCst) == 0).await;
    assert_eq!(backend.input_held.load(Ordering::SeqCst), 0);

    let mut replacement = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    replacement.next_frame(WAIT).await.unwrap();
    assert!(previous_control.send(press.clone()).await.is_err());
    assert!(
        replacement.send_input(press).await.is_err(),
        "new viewer must remain view-only"
    );
    assert_eq!(backend.input_active.load(Ordering::SeqCst), 0);
    assert_eq!(backend.input_held.load(Ordering::SeqCst), 0);
    server.legacy().await;
    replacement.set_input_enabled(true).await.unwrap();
    assert_eq!(
        backend.input_held.load(Ordering::SeqCst),
        0,
        "old press must never replay"
    );
    replacement.close().await.unwrap();
}

#[path="takeover_sessions/mod.rs"]
mod takeover_sessions;
