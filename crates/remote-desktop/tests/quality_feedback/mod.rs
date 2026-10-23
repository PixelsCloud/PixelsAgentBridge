use super::*;

#[tokio::test]
async fn pressure_changes_encoder_budget_and_quality_is_owned_generation_bound_and_optional() {
    let budget = Arc::new(AtomicU32::new(0));
    let backend = Arc::new(Backend { budget: Some(budget.clone()), ..Default::default() });
    let global = DesktopHost::new(Some(backend.clone()));
    let server = Harness::new(&global).await;
    let other = Harness::new(&global).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT).await.unwrap();
    let first = viewer.next_frame(WAIT).await.unwrap();
    viewer.report_presented(&first.header).unwrap();
    let mut quality = viewer.quality_control();
    assert_eq!(quality.snapshot().await.unwrap().encoder_target_bps, Some(0));
    let config = viewer.opened().config.clone();
    let query = RemoteDesktopRequest::Quality { version: DESKTOP_QUALITY_SCHEMA_VERSION,
        session_id: config.session_id, generation: config.generation };
    assert!(matches!(other.request(query.clone()).await,
        RemoteDesktopReply::Error { code: DesktopErrorCode::WrongOwner, .. }));
    let mut last = viewer.next_frame(WAIT).await.unwrap().header;
    for _ in 1..DESKTOP_MAX_IN_FLIGHT { last = viewer.next_frame(WAIT).await.unwrap().header; }
    tokio::time::sleep(Duration::from_millis(900)).await;
    // Polling cannot grant capture credit or stop another reliable stream.
    let before = backend.produced.load(Ordering::SeqCst);
    assert_eq!(quality.snapshot().await.unwrap().encoder_target_bps, Some(0));
    assert_eq!(backend.produced.load(Ordering::SeqCst), before);
    server.legacy().await;
    viewer.report_presented(&last).unwrap();
    let next = viewer.next_frame(WAIT).await.unwrap();
    assert_eq!(next.header.frame_id, last.frame_id + 1);
    assert_eq!(budget.load(Ordering::SeqCst), 3_000_000);
    let reduced = quality.snapshot().await.unwrap();
    assert_eq!(reduced.encoder_target_bps, Some(3_000_000));
    assert!(reduced.capture_fps_milli < 15_000);
    viewer.select_screen("left".into()).await.unwrap();
    assert!(quality.snapshot().await.is_err());
    assert!(matches!(server.request(query).await,
        RemoteDesktopReply::Error { code: DesktopErrorCode::StaleGeneration, .. }));
    let replacement = viewer.next_frame(WAIT).await.unwrap();
    assert_eq!(replacement.header.generation, 2);
    assert_eq!(replacement.header.frame_id, 1);
    let inherited = viewer.quality_control().snapshot().await.unwrap();
    assert_eq!(inherited.encoder_target_bps, Some(3_000_000));
    assert!(inherited.capture_fps_milli < 15_000);
    viewer.close().await.unwrap();
    server.legacy().await;
}

#[tokio::test]
async fn a_source_without_budget_control_keeps_working_with_quality_polling() {
    let global = DesktopHost::new(Some(Arc::new(Backend::default())));
    let server = Harness::new(&global).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT).await.unwrap();
    let frame = viewer.next_frame(WAIT).await.unwrap();
    viewer.report_presented(&frame.header).unwrap();
    assert_eq!(viewer.quality_control().snapshot().await.unwrap().encoder_target_bps, None);
    viewer.close().await.unwrap();
}

#[tokio::test]
async fn pipeline_diagnostics_are_owned_read_only_and_survive_static_capture() {
    let wake = Arc::new(tokio::sync::Notify::new());
    let backend = Arc::new(Backend { wake: Some(wake.clone()), ..Default::default() });
    let global = DesktopHost::new(Some(backend.clone()));
    let server = Harness::new(&global).await;
    let other = Harness::new(&global).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT).await.unwrap();
    let frame = viewer.next_frame(WAIT).await.unwrap();
    let mut diagnostic = viewer.quality_control();
    let first = diagnostic.diagnostics().await.unwrap();
    assert_eq!(first.sent_frame_id, 1); assert_eq!(first.presented_frame_id, 0);
    assert_eq!(first.produced_frames, 1); assert_eq!(first.encoded_bytes, frame.bytes.len() as u64);
    assert!(first.oldest_unpresented_ms.is_some());
    assert!(first.send_last_us.is_some()); assert!(first.source_last_us.is_some());
    // A source without native measurements must report unknown, not zero.
    assert_eq!(first.capture, None);
    let query = RemoteDesktopRequest::Diagnostics { version: DESKTOP_DIAGNOSTICS_VERSION,
        session_id: first.session_id, generation: first.generation };
    assert!(matches!(other.request(query.clone()).await,
        RemoteDesktopReply::Error { code: DesktopErrorCode::WrongOwner, .. }));
    viewer.report_presented(&frame.header).unwrap();
    tokio::time::timeout(WAIT, async {
        loop {
            let snapshot = diagnostic.diagnostics().await.unwrap();
            if snapshot.presented_frame_id == 1 && snapshot.stage == DesktopProducerStage::Capture {
                assert!(snapshot.presentation_ack_last_us.is_some());
                assert_eq!(snapshot.oldest_unpresented_ms, None);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let idle = diagnostic.diagnostics().await.unwrap();
    assert_eq!(idle.produced_frames, 1); assert!(idle.stage_age_ms >= 90);
    assert_eq!(backend.produced.load(Ordering::SeqCst), 1);
    server.legacy().await;
    wake.notify_one();
    let next = viewer.next_frame(WAIT).await.unwrap();
    assert_eq!(next.header.frame_id, 2);
    viewer.select_screen("left".into()).await.unwrap();
    assert!(diagnostic.diagnostics().await.is_err());
    assert!(matches!(server.request(query).await,
        RemoteDesktopReply::Error { code: DesktopErrorCode::StaleGeneration, .. }));
    viewer.close().await.unwrap();
}
