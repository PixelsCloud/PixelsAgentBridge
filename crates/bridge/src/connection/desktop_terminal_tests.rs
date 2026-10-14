use super::*;
use pab_protocol::{
    DesktopTerminalControl, DesktopTerminalEvent, DesktopTerminalReply, DesktopTerminalRequest,
    ExecutionIdentity, ExecutionSelection,
};
use sha2::Digest;
use std::time::Duration;
const WAIT: Duration = Duration::from_secs(10);
async fn pair() -> (
    PabEndpoint,
    PabEndpoint,
    AuthenticatedDeviceConnection,
    PabConnection,
) {
    use pab_protocol::{DeviceId, TenantId};
    let config = PabEndpointConfig::new(vec!["https://127.0.0.1:1".parse().unwrap()]).unwrap();
    let key = || {
        iroh_base::SecretKey::from_bytes(
            &sha2::Sha256::digest(RequestId::new().to_string().as_bytes()).into(),
        )
    };
    let a = PabEndpoint::bind(config.clone(), key()).await.unwrap();
    let b = PabEndpoint::bind(config, key()).await.unwrap();
    let mut addresses = b.watch_address();
    tokio::time::timeout(WAIT, async {
        while addresses.borrow().direct_addresses.is_empty() {
            addresses.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let direct = addresses
        .borrow()
        .direct_addresses
        .iter()
        .map(|address| {
            std::net::SocketAddr::new(
                if address.is_ipv4() {
                    std::net::Ipv4Addr::LOCALHOST.into()
                } else {
                    std::net::Ipv6Addr::LOCALHOST.into()
                },
                address.port(),
            )
        })
        .collect();
    let remote = b.clone();
    let accepted = tokio::spawn(async move { remote.accept().await.unwrap().unwrap() });
    let connection = a
        .connect(
            *b.id().as_bytes(),
            &PabEndpointAddress {
                direct_addresses: direct,
                relay_urls: vec![],
            },
            WAIT,
        )
        .await
        .unwrap();
    let server = accepted.await.unwrap();
    let client = AuthenticatedDeviceConnection::test_new(
        connection,
        DeviceRef {
            tenant_id: TenantId::from_u128(1),
            device_id: DeviceId::from_u128(2),
        },
    );
    (a, b, client, server)
}

#[tokio::test]
async fn close_after_natural_exit_is_idempotent_and_other_errors_remain_visible() {
    let (_a, _b, client, server) = pair().await;
    let session_id = RequestId::new();
    let peer = tokio::spawn(async move {
        for response in [
            DeviceTaskResponse::Error {
                code: DeviceTaskErrorCode::NotFound,
                message: "worker already reclaimed after natural exit".into(),
            },
            DeviceTaskResponse::Error {
                code: DeviceTaskErrorCode::NotFound,
                message: "repeated close".into(),
            },
            DeviceTaskResponse::Error {
                code: DeviceTaskErrorCode::AccessDenied,
                message: "another owner".into(),
            },
            DeviceTaskResponse::DesktopTerminal {
                reply: DesktopTerminalReply::Closed,
            },
        ] {
            let mut stream = server.accept_bi(WAIT).await.unwrap();
            assert!(matches!(
                stream.receive_json::<DeviceTaskRequest>(WAIT).await.unwrap(),
                DeviceTaskRequest::DesktopTerminal {
                    request: DesktopTerminalRequest::Close { session_id: id }, ..
                } if id == session_id
            ));
            stream.send_json(&response, WAIT).await.unwrap();
        }
    });
    client.close_desktop_terminal_id(session_id).await.unwrap();
    client.close_desktop_terminal_id(session_id).await.unwrap();
    assert!(matches!(
        client.close_desktop_terminal_id(session_id).await,
        Err(BridgeError::RemoteTask {
            code: DeviceTaskErrorCode::AccessDenied,
            ..
        })
    ));
    // A close rejection must not destroy the shared authenticated connection.
    client.close_desktop_terminal_id(session_id).await.unwrap();
    peer.await.unwrap();
}

#[tokio::test]
async fn unsupported_device_never_opens_a_legacy_terminal() {
    let (_a, _b, client, server) = pair().await;
    let peer = tokio::spawn(async move {
        let mut stream = server.accept_bi(WAIT).await.unwrap();
        let request: DeviceTaskRequest = stream.receive_json(WAIT).await.unwrap();
        assert!(matches!(request, DeviceTaskRequest::GetEnvironment { .. }));
        stream
            .send_json(
                &DeviceTaskResponse::Error {
                    code: DeviceTaskErrorCode::Unsupported,
                    message: "old fixture".into(),
                },
                WAIT,
            )
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(150), server.accept_bi(WAIT))
                .await
                .is_err()
        );
    });
    assert!(
        client
            .open_desktop_terminal(ExecutionSelection::default(), 100, 40)
            .await
            .is_err()
    );
    peer.await.unwrap();
}
#[tokio::test]
async fn real_quic_client_streams_bytes_checks_offsets_and_uses_independent_close() {
    let (_a, _b, client, server) = pair().await;
    let device = client.device_ref;
    let payload: Vec<u8> = (0..70000).map(|i| (i % 251) as u8).collect();
    let expected = payload.clone();
    let peer = tokio::spawn(async move {
        let mut stream = server.accept_bi(WAIT).await.unwrap();
        assert!(matches!(
            stream
                .receive_json::<DeviceTaskRequest>(WAIT)
                .await
                .unwrap(),
            DeviceTaskRequest::GetEnvironment { .. }
        ));
        let context = serde_json::json!({"device_ref": device, "execution": {"os_family":"windows","os_name":"Windows","os_version":"test","architecture":"x86_64","execution_scope":"native","path_style":"windows","interpreter":null,"cwd":"C:\\test","environment_revision":"fixture"}, "source":"executor_verified","observed_at_unix_ms":1,"freshness":"current"});
        let environment: DeviceTaskResponse = serde_json::from_value(serde_json::json!({"type":"environment","context": context,"desktop_terminal_schema_version":1})).unwrap();
        stream.send_json(&environment, WAIT).await.unwrap();
        let mut stream = server.accept_bi(WAIT).await.unwrap();
        let DeviceTaskRequest::DesktopTerminal {
            request:
                DesktopTerminalRequest::Open {
                    session_id,
                    cols,
                    rows,
                    ..
                },
            ..
        } = stream.receive_json(WAIT).await.unwrap()
        else {
            panic!("new open");
        };
        let identity: ExecutionIdentity = serde_json::from_value(serde_json::json!({"mode":"service","account_id":"S-1-5-18","account_name":"SYSTEM","home":"C:\\test","primary_group":null,"session_id":"0","logon_id":"fixture","environment_source":"service_process"})).unwrap();
        let opened = pab_protocol::DesktopTerminalOpened {
            session_id,
            cols,
            rows,
            shell: "powershell.exe".into(),
            arguments: vec![],
            cwd: "C:\\test".into(),
            identity,
        };
        stream
            .send_json(
                &DeviceTaskResponse::DesktopTerminal {
                    reply: DesktopTerminalReply::Opened { opened },
                },
                WAIT,
            )
            .await
            .unwrap();
        let mut control = server.accept_bi(WAIT).await.unwrap();
        assert!(
            matches!(control.receive_json::<DeviceTaskRequest>(WAIT).await.unwrap(), DeviceTaskRequest::DesktopTerminal { request: DesktopTerminalRequest::Control { session_id: id }, .. } if id == session_id)
        );
        control
            .send_frame_json(
                &DeviceTaskResponse::DesktopTerminal {
                    reply: DesktopTerminalReply::Ready,
                },
                WAIT,
            )
            .await
            .unwrap();
        let (mut control_writer, mut control_reader) = control.into_framed();
        let control_task = tokio::spawn(async move {
            let bytes = control_reader.read(24 * 1024).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<DesktopTerminalControl>(&bytes).unwrap(),
                DesktopTerminalControl::Input {
                    sequence: 1,
                    bytes: "中文\r".as_bytes().to_vec()
                }
            );
            control_writer
                .write(
                    &serde_json::to_vec(&DesktopTerminalReply::Applied).unwrap(),
                    4096,
                )
                .await
                .unwrap();
            let bytes = control_reader.read(24 * 1024).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<DesktopTerminalControl>(&bytes).unwrap(),
                DesktopTerminalControl::Resize {
                    cols: 120,
                    rows: 50
                }
            );
            control_writer
                .write(
                    &serde_json::to_vec(&DesktopTerminalReply::Applied).unwrap(),
                    4096,
                )
                .await
                .unwrap();
        });
        let mut output = server.accept_bi(WAIT).await.unwrap();
        assert!(
            matches!(output.receive_json::<DeviceTaskRequest>(WAIT).await.unwrap(), DeviceTaskRequest::DesktopTerminal { request: DesktopTerminalRequest::Output { session_id: id }, .. } if id == session_id)
        );
        output
            .send_frame_json(
                &DeviceTaskResponse::DesktopTerminal {
                    reply: DesktopTerminalReply::Ready,
                },
                WAIT,
            )
            .await
            .unwrap();
        let (mut writer, mut reader) = output.into_framed();
        let mut offset = 0_u64;
        for bytes in payload.chunks(16384) {
            let mut frame = vec![0];
            frame.extend_from_slice(&offset.to_be_bytes());
            frame.extend_from_slice(bytes);
            writer.write(&frame, 16393).await.unwrap();
            offset += bytes.len() as u64;
            assert_eq!(reader.read(8).await.unwrap(), offset.to_be_bytes());
        }
        let mut end = vec![1];
        end.extend_from_slice(&offset.to_be_bytes());
        end.extend_from_slice(br#"{"exit_code":0,"reason":"Shell exited"}"#);
        writer.write(&end, 4105).await.unwrap();
        control_task.await.unwrap();
        let mut close = server.accept_bi(WAIT).await.unwrap();
        assert!(
            matches!(close.receive_json::<DeviceTaskRequest>(WAIT).await.unwrap(), DeviceTaskRequest::DesktopTerminal { request: DesktopTerminalRequest::Close { session_id: id }, .. } if id == session_id)
        );
        close
            .send_json(
                &DeviceTaskResponse::DesktopTerminal {
                    reply: DesktopTerminalReply::Closed,
                },
                WAIT,
            )
            .await
            .unwrap();
    });
    let session = client
        .open_desktop_terminal(Default::default(), 100, 40)
        .await
        .unwrap();
    session.input("中文\r".as_bytes().to_vec()).await.unwrap();
    session.resize(120, 50).await.unwrap();
    assert!(session.consume(1).await.is_err());
    let mut all = Vec::new();
    loop {
        match session.next_event().await.unwrap() {
            DesktopTerminalEvent::Output { offset, bytes } => {
                assert_eq!(offset, all.len() as u64);
                all.extend(bytes);
                session.consume(all.len() as u64).await.unwrap();
                session.consume(all.len() as u64).await.unwrap();
            }
            DesktopTerminalEvent::Ended {
                offset, exit_code, ..
            } => {
                assert_eq!(offset, all.len() as u64);
                assert_eq!(exit_code, Some(0));
                break;
            }
        }
    }
    assert_eq!(all, expected);
    assert!(session.consume(0).await.is_err());
    session.close().await.unwrap();
    session.close().await.unwrap();
    peer.await.unwrap();
}
