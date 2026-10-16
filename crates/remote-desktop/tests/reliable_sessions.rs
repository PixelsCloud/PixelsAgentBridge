//! Actual Iroh/QUIC streams on loopback. The actor represents the existing
//! authentication result; these tests do not claim to test password auth or
//! native H.264. Payloads are deterministic transport fixtures, not real video.
use pab_protocol::*;
use pab_remote_desktop::*;
use pab_transport::*;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
const WAIT: Duration = Duration::from_secs(5);

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
    active: Arc<AtomicUsize>,
    produced: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
}
struct Source {
    backend: Arc<AtomicUsize>,
    produced: Arc<AtomicUsize>,
    number: u64,
    marker: u8,
}
impl Drop for Source {
    fn drop(&mut self) {
        self.backend.fetch_sub(1, Ordering::SeqCst);
    }
}
impl DesktopSource for Source {
    fn next_frame(&mut self) -> MediaFuture<'_, MediaFrame> {
        Box::pin(async move {
            tokio::task::yield_now().await;
            self.number += 1;
            self.produced.fetch_add(1, Ordering::SeqCst);
            Ok(MediaFrame {
                bytes: vec![self.marker; 512 * 1024 + 13],
                capture_time_us: self.number * 66667,
                keyframe: self.number == 1,
            })
        })
    }
}
impl DesktopBackend for Backend {
    fn screens(&self) -> MediaFuture<'_, Vec<DesktopScreen>> {
        Box::pin(async { Ok(screens()) })
    }
    fn start(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopSource>> {
        Box::pin(async move {
            let count = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(count, Ordering::SeqCst);
            Ok(Box::new(Source {
                backend: self.active.clone(),
                produced: self.produced.clone(),
                number: 0,
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
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
        self.client.close(b"isolated test ended");
    }
}
impl Harness {
    async fn new(global: &DesktopHost) -> Self {
        let (a, b, client, peer) = pair().await;
        let server = peer.clone();
        let host = global.for_connection();
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
                            let _ = host.serve(actor(), None, request, stream, WAIT).await;
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
    assert_eq!(viewer.opened().config.generation, 2);
    assert_eq!(viewer.opened().config.screen.x, -960);
    assert_eq!(viewer.opened().config.screen.scale_milli, 2000);
    let switched = viewer.next_frame(WAIT).await.unwrap();
    assert_eq!(switched.header.frame_id, 1);
    assert!(switched.header.keyframe);
    assert_eq!(switched.bytes, vec![0x55; 512 * 1024 + 13]);
    assert!(viewer.report_presented(&old_header).is_err());
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
            version: 1,
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
        version: 1,
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
            version: 1,
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
