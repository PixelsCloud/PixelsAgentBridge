//! Real reliable streams; deterministic cursor images, no native screen/input.
use super::{WAIT, config, pair};
use pab_protocol::*;
use pab_remote_desktop::*;
use pab_transport::*;
use std::time::Duration;

fn image(config: &DesktopVideoConfig) -> CursorUpdate {
    CursorUpdate {
        header: DesktopCursorHeader {
            version: DESKTOP_CURSOR_SCHEMA_VERSION,
            session_id: config.session_id,
            generation: config.generation,
            sequence: 1,
            position: Some(DesktopCursorPosition { x: -720, y: -50 }),
            image_id: 1,
            image: Some(DesktopCursorImage {
                format: DesktopCursorFormat::AndXorBgra,
                width: 512,
                height: 512,
                hotspot_x: 511,
                hotspot_y: 0,
            }),
        },
        bytes: (0..DESKTOP_MAX_CURSOR_BYTES)
            .map(|i| (i % 251) as u8)
            .collect(),
    }
}
async fn streams(client: &PabConnection, peer: &PabConnection) -> (PabBiStream, PabBiStream) {
    let mut sent = client.open_bi(WAIT).await.unwrap();
    sent.send_frame_json(&"cursor-test", WAIT).await.unwrap();
    let mut received = peer.accept_bi(WAIT).await.unwrap();
    assert_eq!(
        received.receive_json::<String>(WAIT).await.unwrap(),
        "cursor-test"
    );
    (sent, received)
}
async fn unaffected_connection(client: &PabConnection, peer: &PabConnection) {
    let mut sent = client.open_bi(WAIT).await.unwrap();
    sent.send_frame_json(&42, WAIT).await.unwrap();
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

#[tokio::test]
async fn cursor_shapes_moves_hidden_and_replacement_share_one_bounded_cache() {
    let (_a, _b, client, peer) = pair().await;
    let mut config = config();
    config.screen = super::screens().remove(1); // negative origin, 2x source scale
    let (sent, received) = streams(&client, &peer).await;
    let (writer, _keep_receiving) = sent.into_framed();
    let (_keep_sending, reader) = received.into_framed();
    let mut writer = DesktopCursorWriter::new(writer, config.clone()).unwrap();
    let mut reader = DesktopCursorReader::new(reader, config.clone()).unwrap();
    let mut update = image(&config);
    for step in 0..5 {
        match step {
            0 => {
                // Hidden before any image is available.
                update.header.position = None;
                update.header.image_id = 0;
                update.header.image = None;
                update.bytes.clear();
            }
            1 => {
                update = image(&config);
                update.header.sequence = 2;
            }
            2 => {
                update.header.image = None;
                update.header.position = Some(DesktopCursorPosition { x: -1, y: 439 });
                update.bytes.clear();
            }
            3 => update.header.position = None,
            4 => {
                update.header.image_id = 2;
                update.header.image = Some(DesktopCursorImage {
                    format: DesktopCursorFormat::PremultipliedBgra,
                    width: 2,
                    height: 1,
                    hotspot_x: 1,
                    hotspot_y: 0,
                });
                update.header.position = Some(DesktopCursorPosition { x: -960, y: -100 });
                update.bytes = vec![0, 128, 0, 128, 0, 0, 255, 255];
            }
            _ => unreachable!(),
        }
        let (sent, received) =
            tokio::join!(writer.send(&update, WAIT), reader.receive_after_idle(WAIT));
        sent.unwrap();
        let received = received.unwrap();
        assert_eq!(received.header, update.header);
        assert_eq!(received.bytes, update.bytes);
        if step == 2 || step == 3 {
            assert!(received.bytes.is_empty());
        }
        update.header.sequence += 1;
    }
    unaffected_connection(&client, &peer).await;
}

#[tokio::test]
async fn cursor_rejects_bad_identity_sequence_shape_and_hotspot_before_payload() {
    let (_a, _b, client, peer) = pair().await;
    for case in 0..9 {
        let config = config();
        let mut header = image(&config).header;
        match case {
            0 => header.session_id = RequestId::new(),
            1 => header.generation += 1,
            2 => header.sequence = 0,
            3 => header.sequence += 1,
            4 => header.image.as_mut().unwrap().width = u32::MAX,
            5 => header.image.as_mut().unwrap().hotspot_y = 512,
            6 => header.image = None, // Image ID is unknown in a new stream.
            7 => header.image_id = 0,
            _ => header.version += 1,
        }
        let mut sent = client.open_bi(WAIT).await.unwrap();
        sent.send_frame_json(&header, WAIT).await.unwrap(); // Deliberately NO pixels.
        let (_writer, reader) = peer.accept_bi(WAIT).await.unwrap().into_framed();
        let mut reader = DesktopCursorReader::new(reader, config).unwrap();
        let result = tokio::time::timeout(WAIT, reader.receive_after_idle(WAIT))
            .await
            .unwrap();
        assert!(
            matches!(result, Err(DesktopError::Protocol(_))),
            "case {case}"
        );
        assert!(matches!(
            reader.receive_after_idle(WAIT).await,
            Err(DesktopError::Closed)
        ));
    }
    unaffected_connection(&client, &peer).await;
}

#[tokio::test]
async fn cursor_cannot_reuse_old_images_or_duplicate_position_sequences() {
    let (_a, _b, client, peer) = pair().await;
    for case in 0..4 {
        let config = config();
        let mut update = image(&config);
        update.header.image_id = 5;
        // Small image keeps the setup independent of transport flow control.
        update.header.image.as_mut().unwrap().width = 1;
        update.header.image.as_mut().unwrap().height = 1;
        update.header.image.as_mut().unwrap().hotspot_x = 0;
        update.bytes = vec![0; 8];
        let mut sent = client.open_bi(WAIT).await.unwrap();
        sent.send_frame_json(&update.header, WAIT).await.unwrap();
        sent.send_binary_frame(&update.bytes, WAIT).await.unwrap();
        let (_writer, incoming) = peer.accept_bi(WAIT).await.unwrap().into_framed();
        let mut reader = DesktopCursorReader::new(incoming, config).unwrap();
        reader.receive_after_idle(WAIT).await.unwrap();
        update.header.sequence = 2;
        match case {
            0 => {} // Resending an image under the same ID is invalid.
            1 => update.header.image_id = 4,
            2 => {
                update.header.image = None;
                update.header.image_id = 6;
            }
            _ => {
                update.header.image = None;
                update.header.sequence = 1;
            }
        }
        sent.send_frame_json(&update.header, WAIT).await.unwrap();
        assert!(matches!(
            reader.receive_after_idle(WAIT).await,
            Err(DesktopError::Protocol(_))
        ));
    }
}

#[tokio::test]
async fn cursor_partial_image_timeout_or_cancel_does_not_close_other_streams() {
    let (_a, _b, client, peer) = pair().await;
    for cancel in [false, true] {
        let config = config();
        let update = image(&config);
        let mut sent = client.open_bi(WAIT).await.unwrap();
        sent.send_frame_json(&update.header, WAIT).await.unwrap();
        sent.send_binary_frame(&[0; 10], WAIT).await.unwrap();
        let (_writer, incoming) = peer.accept_bi(WAIT).await.unwrap().into_framed();
        let mut reader = DesktopCursorReader::new(incoming, config).unwrap();
        if cancel {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(30)) => {},
                _ = reader.receive_after_idle(WAIT) => panic!("partial cursor must remain pending"),
            }
        } else {
            assert!(matches!(
                reader.receive_after_idle(Duration::from_millis(30)).await,
                Err(DesktopError::Timeout)
            ));
        }
        assert!(matches!(
            reader.receive_after_idle(WAIT).await,
            Err(DesktopError::Closed)
        ));
        unaffected_connection(&client, &peer).await;
    }
}

#[tokio::test]
async fn cursor_stationary_idle_is_allowed_but_cancelled_idle_reader_is_retired() {
    let (_a, _b, client, peer) = pair().await;
    let config = config();
    let (mut sent, received) = streams(&client, &peer).await;
    let (_writer, incoming) = received.into_framed();
    let mut reader = DesktopCursorReader::new(incoming, config.clone()).unwrap();
    let waiting = tokio::spawn(async move {
        let result = reader.receive_after_idle(Duration::from_millis(100)).await;
        (reader, result)
    });
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(!waiting.is_finished());
    let mut update = image(&config);
    update.header.image_id = 0;
    update.header.image = None;
    update.header.position = None;
    sent.send_frame_json(&update.header, WAIT).await.unwrap();
    let (mut reader, result) = tokio::time::timeout(WAIT, waiting).await.unwrap().unwrap();
    assert!(result.unwrap().bytes.is_empty());
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(30)) => {},
        _ = reader.receive_after_idle(WAIT) => panic!("idle reader must remain pending"),
    }
    assert!(matches!(
        reader.receive_after_idle(WAIT).await,
        Err(DesktopError::Closed)
    ));
    unaffected_connection(&client, &peer).await;
}

#[tokio::test]
async fn cursor_writer_validates_before_io_and_retires_after_io_failure() {
    let (_a, _b, client, peer) = pair().await;
    let config = config();
    let (sent, received) = streams(&client, &peer).await;
    let (outgoing, _reader) = sent.into_framed();
    let (_writer, incoming) = received.into_framed();
    let mut writer = DesktopCursorWriter::new(outgoing, config.clone()).unwrap();
    let mut reader = DesktopCursorReader::new(incoming, config.clone()).unwrap();
    let mut update = image(&config);
    update.bytes.pop();
    assert!(matches!(
        writer.send(&update, WAIT).await,
        Err(DesktopError::Protocol(_))
    ));
    update = image(&config);
    assert!(matches!(
        writer.send(&update, Duration::ZERO).await,
        Err(DesktopError::Protocol(_))
    ));
    let (sent, received) =
        tokio::join!(writer.send(&update, WAIT), reader.receive_after_idle(WAIT));
    sent.unwrap();
    assert_eq!(received.unwrap().bytes, update.bytes);
    update.header.sequence += 1;
    update.header.image = None;
    update.bytes.clear();
    peer.close(b"cursor failure test");
    tokio::time::timeout(WAIT, client.closed()).await.unwrap();
    assert!(writer.send(&update, WAIT).await.is_err());
    assert!(matches!(
        writer.send(&update, WAIT).await,
        Err(DesktopError::Closed)
    ));
}
