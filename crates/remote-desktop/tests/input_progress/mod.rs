use super::*;

#[tokio::test]
async fn slow_input_does_not_block_display_acks_or_reorder_input() {
    let block = Arc::new(tokio::sync::Notify::new());
    let backend = Arc::new(Backend {
        input_block: Some(block.clone()),
        ..Default::default()
    });
    let global = DesktopHost::new(Some(backend.clone()));
    let server = Harness::new(&global).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 60, WAIT)
        .await
        .unwrap();
    let first = viewer.next_frame(WAIT).await.unwrap();
    viewer.report_presented(&first.header).unwrap();
    viewer.set_input_enabled(true).await.unwrap();
    let input = viewer.input_control();
    let held = tokio::spawn(async move {
        input
            .send(RemoteDesktopInput::Key {
                virtual_key: 17,
                down: true,
            })
            .await
    });
    eventually(|| backend.input_applied.load(Ordering::SeqCst) == 1).await;
    let input = viewer.input_control();
    let release = tokio::spawn(async move { input.send(RemoteDesktopInput::ReleaseAll).await });
    // More frames/ACKs than either video credit or the client ACK window can
    // hold: proves replies also bypass the pending native input operation.
    for _ in 0..20 {
        let frame = viewer.next_frame(Duration::from_secs(2)).await.unwrap();
        viewer.report_presented(&frame.header).unwrap();
    }
    assert_eq!(backend.input_applied.load(Ordering::SeqCst), 1);
    assert!(!held.is_finished());
    assert!(!release.is_finished());
    assert!(
        viewer
            .presentation_control()
            .trace()
            .iter()
            .any(|ack| ack.frame_id > 8 && ack.reply_us.is_some())
    );
    block.notify_one();
    held.await.unwrap().unwrap();
    eventually(|| backend.input_applied.load(Ordering::SeqCst) == 2).await;
    block.notify_one();
    release.await.unwrap().unwrap();
    assert_eq!(backend.input_held.load(Ordering::SeqCst), 0);
    viewer.close().await.unwrap();
    server.legacy().await;
}

#[tokio::test]
async fn closing_during_slow_input_releases_keys_and_discards_queued_mutations() {
    let backend = Arc::new(Backend {
        input_block: Some(Arc::new(tokio::sync::Notify::new())),
        ..Default::default()
    });
    let global = DesktopHost::new(Some(backend.clone()));
    let server = Harness::new(&global).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 30, WAIT)
        .await
        .unwrap();
    let frame = viewer.next_frame(WAIT).await.unwrap();
    viewer.report_presented(&frame.header).unwrap();
    viewer.set_input_enabled(true).await.unwrap();
    let input = viewer.input_control();
    let pending = tokio::spawn(async move {
        input
            .send(RemoteDesktopInput::Key {
                virtual_key: 17,
                down: true,
            })
            .await
    });
    eventually(|| backend.input_applied.load(Ordering::SeqCst) == 1).await;
    let input = viewer.input_control();
    let queued = tokio::spawn(async move {
        input
            .send(RemoteDesktopInput::Key {
                virtual_key: 65,
                down: true,
            })
            .await
    });
    viewer.close().await.unwrap();
    assert!(pending.await.unwrap().is_err());
    assert!(queued.await.unwrap().is_err());
    eventually(|| backend.input_active.load(Ordering::SeqCst) == 0).await;
    assert_eq!(backend.input_held.load(Ordering::SeqCst), 0);
    assert_eq!(backend.input_applied.load(Ordering::SeqCst), 1);
}
