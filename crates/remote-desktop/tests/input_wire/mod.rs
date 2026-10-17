//! Deliberately bypass the client's sequence generator over real QUIC streams.
//! Counts refer to the test input sink, not OS injection or application actions.
use super::*;

async fn attach(server: &Harness, request: RemoteDesktopRequest) -> PabBiStream {
    let mut stream = server.client.open_bi(WAIT).await.unwrap();
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
    assert!(matches!(
        stream.receive_json(WAIT).await.unwrap(),
        DeviceTaskResponse::RemoteDesktop {
            reply: RemoteDesktopReply::Ready
        }
    ));
    stream
}

async fn open_raw(
    server: &Harness,
) -> (
    DesktopVideoConfig,
    PabBiStream,
    PabFrameWriter,
    DesktopVideoReader,
) {
    let session_id = RequestId::new();
    let RemoteDesktopReply::Opened { opened } = server
        .request(RemoteDesktopRequest::Open {
            version: REMOTE_DESKTOP_SCHEMA_VERSION,
            session_id,
            screen_id: None,
            fps: 15,
        })
        .await
    else {
        panic!("raw session did not open");
    };
    let control = attach(server, RemoteDesktopRequest::Control { session_id }).await;
    let video = attach(
        server,
        RemoteDesktopRequest::Video {
            session_id,
            generation: opened.config.generation,
        },
    )
    .await;
    let (keepalive, reader) = video.into_framed();
    let mut reader = DesktopVideoReader::new(reader, opened.config.clone()).unwrap();
    reader.receive(WAIT).await.unwrap();
    (opened.config, control, keepalive, reader)
}

async fn command(
    stream: &mut PabBiStream,
    sequence: u64,
    command: DesktopControlCommand,
) -> DesktopControlReply {
    stream
        .send_frame_json(&DesktopControl { sequence, command }, WAIT)
        .await
        .unwrap();
    stream.receive_json(WAIT).await.unwrap()
}

fn press(generation: u64) -> DesktopControlCommand {
    DesktopControlCommand::Input {
        generation,
        event: RemoteDesktopInput::Key {
            virtual_key: 0x11,
            down: true,
        },
    }
}

#[tokio::test]
async fn duplicate_or_skipped_sequence_releases_input_without_replay_or_connection_loss() {
    // The next valid sequence after enable/press is 3. Include zero, old,
    // duplicate, skipped and overflowing values supplied by a malformed peer.
    for invalid in [0, 1, 2, 4, u64::MAX] {
        let backend = Arc::new(Backend::default());
        let global = DesktopHost::new(Some(backend.clone()));
        let server = Harness::new(&global).await;
        let (config, mut control, _keepalive, _video) = open_raw(&server).await;
        assert_eq!(
            command(
                &mut control,
                1,
                DesktopControlCommand::SetInput {
                    generation: config.generation,
                    enabled: true,
                }
            )
            .await,
            DesktopControlReply::Applied { sequence: 1 }
        );
        assert_eq!(
            command(&mut control, 2, press(config.generation)).await,
            DesktopControlReply::Applied { sequence: 2 }
        );
        assert_eq!(backend.input_held.load(Ordering::SeqCst), 1);
        control
            .send_frame_json(
                &DesktopControl {
                    sequence: invalid,
                    command: press(config.generation),
                },
                WAIT,
            )
            .await
            .unwrap();
        assert!(
            control
                .receive_json::<DesktopControlReply>(WAIT)
                .await
                .is_err()
        );
        eventually(|| {
            backend.input_active.load(Ordering::SeqCst) == 0
                && backend.active.load(Ordering::SeqCst) == 0
        })
        .await;
        assert_eq!(backend.input_held.load(Ordering::SeqCst), 0);
        assert_eq!(backend.input_applied.load(Ordering::SeqCst), 1);
        server.legacy().await;

        let mut replacement = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT)
            .await
            .unwrap();
        assert_ne!(replacement.opened().config.session_id, config.session_id);
        replacement.next_frame(WAIT).await.unwrap();
        let input = replacement.input_control();
        assert!(
            input
                .send(RemoteDesktopInput::Key {
                    virtual_key: 0x11,
                    down: true
                })
                .await
                .is_err()
        );
        input.set_enabled(true).await.unwrap();
        assert_eq!(backend.input_held.load(Ordering::SeqCst), 0);
        assert_eq!(backend.input_applied.load(Ordering::SeqCst), 1);
        replacement.close().await.unwrap();
        eventually(|| {
            backend.input_active.load(Ordering::SeqCst) == 0
                && backend.active.load(Ordering::SeqCst) == 0
        })
        .await;
    }
}

#[tokio::test]
async fn stale_generation_input_is_rejected_without_disabling_current_control() {
    let backend = Arc::new(Backend::default());
    let global = DesktopHost::new(Some(backend.clone()));
    let server = Harness::new(&global).await;
    let (config, mut control, _keepalive, _video) = open_raw(&server).await;
    assert_eq!(
        command(
            &mut control,
            1,
            DesktopControlCommand::SetInput {
                generation: config.generation,
                enabled: true,
            }
        )
        .await,
        DesktopControlReply::Applied { sequence: 1 }
    );
    for (sequence, generation) in [(2, 0), (3, config.generation + 1)] {
        assert!(
            matches!(command(&mut control, sequence, press(generation)).await,
            DesktopControlReply::Error { sequence: actual, code: DesktopErrorCode::StaleGeneration, .. }
            if actual == sequence)
        );
        assert_eq!(backend.input_applied.load(Ordering::SeqCst), 0);
        assert_eq!(backend.input_held.load(Ordering::SeqCst), 0);
    }
    assert_eq!(
        command(&mut control, 4, press(config.generation)).await,
        DesktopControlReply::Applied { sequence: 4 }
    );
    assert_eq!(backend.input_applied.load(Ordering::SeqCst), 1);
    assert_eq!(backend.input_held.load(Ordering::SeqCst), 1);
    drop(control);
    eventually(|| {
        backend.input_active.load(Ordering::SeqCst) == 0
            && backend.active.load(Ordering::SeqCst) == 0
    })
    .await;
    assert_eq!(backend.input_held.load(Ordering::SeqCst), 0);
    server.legacy().await;
}

#[tokio::test]
async fn unsupported_desktop_version_cannot_reserve_the_viewer_or_enable_input() {
    let backend = Arc::new(Backend::default());
    let global = DesktopHost::new(Some(backend.clone()));
    let server = Harness::new(&global).await;
    for version in [
        0,
        REMOTE_DESKTOP_SCHEMA_VERSION - 1,
        REMOTE_DESKTOP_SCHEMA_VERSION + 1,
    ] {
        let session_id = RequestId::new();
        assert!(matches!(
            server
                .request(RemoteDesktopRequest::Open {
                    version,
                    session_id,
                    screen_id: None,
                    fps: 15,
                })
                .await,
            RemoteDesktopReply::Error {
                code: DesktopErrorCode::Unsupported,
                ..
            }
        ));
        assert!(matches!(
            server
                .request(RemoteDesktopRequest::Control { session_id })
                .await,
            RemoteDesktopReply::Error {
                code: DesktopErrorCode::NotFound,
                ..
            }
        ));
    }
    assert_eq!(backend.active.load(Ordering::SeqCst), 0);
    assert_eq!(backend.input_applied.load(Ordering::SeqCst), 0);
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    viewer.next_frame(WAIT).await.unwrap();
    assert_eq!(backend.input_active.load(Ordering::SeqCst), 0);
    viewer.close().await.unwrap();
    eventually(|| backend.active.load(Ordering::SeqCst) == 0).await;
    server.legacy().await;
}
