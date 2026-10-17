use super::*;

#[tokio::test]
async fn delayed_consumption_paces_new_capture_instead_of_bursting_returned_credit() {
    let backend = Arc::new(Backend::default());
    let global = DesktopHost::new(Some(backend));
    let server = Harness::new(&global).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    let mut last = viewer.next_frame(WAIT).await.unwrap().header;
    let start = tokio::time::Instant::now();
    for _ in 1..DESKTOP_MAX_IN_FLIGHT {
        last = viewer.next_frame(WAIT).await.unwrap().header;
    }
    tokio::time::sleep_until(start + Duration::from_millis(900)).await;
    viewer.report_presented(&last).unwrap();
    let fifth = viewer.next_frame(WAIT).await.unwrap();
    let resumed = tokio::time::Instant::now();
    let sixth = viewer.next_frame(WAIT).await.unwrap();
    assert!(
        resumed.elapsed() >= Duration::from_millis(120),
        "returned credits burst at the old fixed cadence"
    );
    assert_eq!((fifth.header.frame_id, sixth.header.frame_id), (5, 6));
    server.legacy().await;
    viewer.close().await.unwrap();
}

#[tokio::test]
async fn healthy_consumer_preserves_requested_cadence_above_twelve_frames_per_second() {
    healthy_cadence(None).await;
}

#[tokio::test]
async fn slow_encoding_is_included_in_cadence_instead_of_delaying_twice() {
    healthy_cadence(Some(Duration::from_millis(60))).await;
}

#[tokio::test]
async fn decoder_pipeline_latency_is_not_mistaken_for_growing_network_backlog() {
    let global = DesktopHost::new(Some(Arc::new(Backend::default())));
    let server = Harness::new(&global).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    let mut pending = std::collections::VecDeque::new();
    let mut start = None;
    for index in 1..=28 {
        pending.push_back(viewer.next_frame(WAIT).await.unwrap().header);
        if pending.len() == DESKTOP_MAX_IN_FLIGHT as usize {
            tokio::time::sleep(Duration::from_millis(20)).await;
            viewer
                .report_presented(&pending.pop_front().unwrap())
                .unwrap();
        }
        if index == 4 {
            start = Some(tokio::time::Instant::now());
        }
    }
    let rate = 24.0 / start.unwrap().elapsed().as_secs_f64();
    assert!(
        rate > 12.0,
        "decoder pipeline caused a pacing feedback loop: {rate}"
    );
    viewer.close().await.unwrap();
}

async fn healthy_cadence(encode_delay: Option<Duration>) {
    let global = DesktopHost::new(Some(Arc::new(Backend {
        encode_delay,
        ..Default::default()
    })));
    let server = Harness::new(&global).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    let first = viewer.next_frame(WAIT).await.unwrap();
    viewer.report_presented(&first.header).unwrap();
    let start = tokio::time::Instant::now();
    for _ in 0..24 {
        let frame = viewer.next_frame(WAIT).await.unwrap();
        viewer.report_presented(&frame.header).unwrap();
    }
    let rate = 24.0 / start.elapsed().as_secs_f64();
    assert!(rate > 12.0 && rate < 16.0, "fixture delivery FPS {rate}");
    viewer.close().await.unwrap();
}

#[tokio::test]
async fn unconsumed_backlog_retires_only_video_and_releases_input_before_new_generation() {
    let backend = Arc::new(Backend::default());
    let global = DesktopHost::new(Some(backend.clone()));
    let server = Harness::new(&global).await;
    let other = Harness::new(&global).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    let old = viewer.presentation_control();
    let old_input = viewer.input_control();
    let mut last = viewer.next_frame(WAIT).await.unwrap().header;
    viewer.set_input_enabled(true).await.unwrap();
    viewer
        .send_input(RemoteDesktopInput::Key {
            virtual_key: 17,
            down: true,
        })
        .await
        .unwrap();
    assert_eq!(backend.input_held.load(Ordering::SeqCst), 1);
    for _ in 1..DESKTOP_MAX_IN_FLIGHT {
        last = viewer.next_frame(WAIT).await.unwrap().header;
    }
    server.legacy().await;
    // Keep control heartbeats alive but never acknowledge the video. The
    // sender must end stale compressed history without dropping the connection.
    assert!(
        tokio::time::timeout(Duration::from_secs(7), viewer.next_frame_when_changed())
            .await
            .unwrap()
            .is_err()
    );
    eventually(|| {
        backend.active.load(Ordering::SeqCst) == 0
            && backend.input_held.load(Ordering::SeqCst) == 0
            && backend.input_active.load(Ordering::SeqCst) == 0
    })
    .await;
    server.legacy().await;
    assert!(matches!(
        RemoteDesktopSession::open(other.client.clone(), None, 15, WAIT).await,
        Err(DesktopError::Rejected {
            code: DesktopErrorCode::Busy,
            ..
        })
    ));
    let previous = viewer.opened().config.clone();
    viewer
        .select_screen(previous.screen.id.clone())
        .await
        .unwrap();
    assert_eq!(viewer.opened().config.session_id, previous.session_id);
    assert_eq!(viewer.opened().config.generation, previous.generation + 1);
    assert!(old.report_presented(&last).is_err());
    assert!(
        old_input
            .send(RemoteDesktopInput::Key {
                virtual_key: 17,
                down: false
            })
            .await
            .is_err()
    );
    let fresh = viewer.next_frame(WAIT).await.unwrap();
    assert_eq!(fresh.header.frame_id, 1);
    assert!(fresh.header.keyframe);
    viewer.report_presented(&fresh.header).unwrap();
    assert_eq!(backend.input_held.load(Ordering::SeqCst), 0);
    assert_eq!(backend.input_active.load(Ordering::SeqCst), 0);
    viewer.close().await.unwrap();
    server.legacy().await;
}

#[tokio::test]
async fn acknowledged_static_video_survives_backlog_deadline_without_extra_capture_frames() {
    let wake = Arc::new(tokio::sync::Notify::new());
    let backend = Arc::new(Backend {
        wake: Some(wake.clone()),
        ..Default::default()
    });
    let global = DesktopHost::new(Some(backend.clone()));
    let server = Harness::new(&global).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    let initial = viewer.next_frame(WAIT).await.unwrap();
    viewer.report_presented(&initial.header).unwrap();
    let reading = viewer.next_frame_when_changed();
    tokio::pin!(reading);
    tokio::select! {
        result = &mut reading => panic!("static capture ended: {:?}", result.err()),
        _ = tokio::time::sleep(Duration::from_millis(5500)) => {},
    }
    assert_eq!(backend.produced.load(Ordering::SeqCst), 1);
    server.legacy().await;
    wake.notify_one();
    let fresh = tokio::time::timeout(WAIT, &mut reading)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fresh.header.frame_id, 2);
    // Session drops normally after this retained receive future is dropped.
}
