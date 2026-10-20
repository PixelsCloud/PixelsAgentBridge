use super::*;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn mobile_jpeg_checks_identity_binary_boundaries_hash_and_codec() {
    let (first, second, client, remote) = crate::connection::test_connection_pair().await;
    for fault in ["none", "wrong_id", "hash", "truncated", "extra", "codec"] {
        let id = RequestId::new();
        let image = pab_screenshot::encode(
            pab_screenshot::image::DynamicImage::new_rgb8(100, 80),
            &ScreenshotOptions::default(),
            None,
            (0, 0),
        )
        .unwrap();
        let device = client.device_ref;
        let server = remote.clone();
        let handler = tokio::spawn(async move {
            let mut environment = server.accept_bi(TIMEOUT).await.unwrap();
            assert!(matches!(
                environment
                    .receive_json::<DeviceTaskRequest>(TIMEOUT)
                    .await
                    .unwrap(),
                DeviceTaskRequest::GetEnvironment { .. }
            ));
            environment
                .send_json(
                    &DeviceTaskResponse::Environment {
                        context: Box::new(TargetContext {
                            device_ref: device,
                            execution: ExecutionContext {
                                os_family: OsFamily::Android,
                                os_name: "Android".into(),
                                os_version: "9".into(),
                                architecture: CpuArchitecture::X86_64,
                                execution_scope: ExecutionScope::Native,
                                identity: None,
                                path_style: PathStyle::Posix,
                                interpreter: None,
                                cwd: None,
                                environment_revision: "android:test".into(),
                            },
                            source: TargetContextSource::ExecutorVerified,
                            observed_at_unix_ms: 1,
                            freshness: ContextFreshness::Current,
                        }),
                        command_schema_version: None,
                        terminal_schema_version: None,
                        desktop_terminal_schema_version: None,
                        filesystem_schema_version: None,
                        transfer_schema_version: None,
                        system_query_schema_version: None,
                        screenshot_schema_version: None,
                        document_schema_version: None,
                        port_forward_schema_version: None,
                        mobile_schema_version: Some(MOBILE_SCHEMA_VERSION),
                    },
                    TIMEOUT,
                )
                .await
                .unwrap();
            let mut stream = server.accept_bi(TIMEOUT).await.unwrap();
            assert!(
                matches!(stream.receive_json::<DeviceTaskRequest>(TIMEOUT).await.unwrap(),DeviceTaskRequest::Mobile {request:MobileRequest::Execute {request_id,..},..} if request_id==id)
            );
            let mut bytes = image.bytes;
            if fault == "codec" {
                bytes.truncate(bytes.len() - 2);
            }
            let reply = MobileReply {
                state: MobileState::Completed,
                image: Some(MobileImage {
                    width: 100,
                    height: 80,
                    size: bytes.len() as u32,
                    sha256: if fault == "hash" {
                        "0".repeat(64)
                    } else {
                        format!("{:x}", Sha256::digest(&bytes))
                    },
                }),
                ..MobileReply::pending(if fault == "wrong_id" {
                    RequestId::new()
                } else {
                    id
                })
            };
            stream
                .send_frame_json(
                    &DeviceTaskResponse::Mobile {
                        reply: Box::new(reply),
                    },
                    TIMEOUT,
                )
                .await
                .unwrap();
            if fault == "truncated" {
                bytes.truncate(bytes.len() / 2);
            }
            let _ = stream.send_binary_frame(&bytes, TIMEOUT).await;
            if fault == "extra" {
                let _ = stream.send_binary_frame(b"extra", TIMEOUT).await;
            }
            let _ = stream.finish_send(TIMEOUT).await;
        });
        let received = client
            .mobile(MobileRequest::Execute {
                request_id: id,
                command: MobileCommand::Screenshot {},
            })
            .await;
        assert_eq!(received.is_ok(), fault == "none", "fault {fault}");
        handler.await.unwrap();
    }
    first.close().await;
    second.close().await;
}
