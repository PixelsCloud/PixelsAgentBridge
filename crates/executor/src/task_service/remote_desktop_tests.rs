use super::*;
use crate::task_service::transfer_tests::{actor, pair, service};
use pab_protocol::{DesktopErrorCode, RemoteDesktopReply, RemoteDesktopRequest, RequestId};
const WAIT: Duration = Duration::from_secs(5);

async fn request(
    svc: &TaskService,
    client: &pab_transport::PabConnection,
    server: &pab_transport::PabConnection,
    request: DeviceTaskRequest,
) -> DeviceTaskResponse {
    let (svc, server) = (svc.clone(), server.clone());
    let task = tokio::spawn(async move {
        svc.handle_stream(actor(), server.accept_bi(WAIT).await.unwrap(), WAIT)
            .await
            .unwrap();
    });
    let mut stream = client.open_bi(WAIT).await.unwrap();
    stream.send_frame_json(&request, WAIT).await.unwrap();
    let reply = stream.receive_json(WAIT).await.unwrap();
    task.await.unwrap();
    reply
}
#[tokio::test]
async fn remote_desktop_unavailable_is_additive_and_presence_keeps_working() {
    let temp = tempfile::tempdir().unwrap();
    let svc = service(temp.path()).await.for_ui_connection();
    let _guard = svc.remote_desktop_connection_guard();
    let (_a, _b, client, server) = pair().await;
    let environment = request(
        &svc,
        &client,
        &server,
        DeviceTaskRequest::GetEnvironment {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
        },
    )
    .await;
    assert!(matches!(
        environment,
        DeviceTaskResponse::Environment {
            remote_desktop_schema_version: None,
            remote_desktop_cursor_schema_version: None,
            terminal_schema_version: Some(2),
            desktop_terminal_schema_version: Some(1),
            ..
        }
    ));
    let unavailable = request(
        &svc,
        &client,
        &server,
        DeviceTaskRequest::RemoteDesktop {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
            request: RemoteDesktopRequest::Open {
                version: 1,
                session_id: RequestId::new(),
                screen_id: None,
                fps: 15,
            },
        },
    )
    .await;
    assert!(matches!(
        unavailable,
        DeviceTaskResponse::RemoteDesktop {
            reply: RemoteDesktopReply::Error {
                code: DesktopErrorCode::Unsupported,
                ..
            }
        }
    ));
    assert!(matches!(
        request(
            &svc,
            &client,
            &server,
            DeviceTaskRequest::GetPresence {
                schema_version: DEVICE_TASK_SCHEMA_VERSION
            }
        )
        .await,
        DeviceTaskResponse::Presence {
            active_operators: 0
        }
    ));
}

struct FixtureBackend;
struct FixtureSource;
struct FixtureCursor(Option<pab_protocol::DesktopVideoConfig>);
impl pab_remote_desktop::DesktopCursorSource for FixtureCursor {
    fn next_update(
        &mut self,
    ) -> pab_remote_desktop::MediaFuture<'_, pab_remote_desktop::CursorUpdate> {
        Box::pin(async move {
            let Some(config) = self.0.take() else {
                return std::future::pending().await;
            };
            Ok(pab_remote_desktop::CursorUpdate {
                header: pab_protocol::DesktopCursorHeader {
                    version: pab_protocol::DESKTOP_CURSOR_SCHEMA_VERSION,
                    session_id: config.session_id,
                    generation: config.generation,
                    sequence: 1,
                    position: Some(pab_protocol::DesktopCursorPosition { x: 20, y: 30 }),
                    image_id: 1,
                    image: Some(pab_protocol::DesktopCursorImage {
                        format: pab_protocol::DesktopCursorFormat::PremultipliedBgra,
                        width: 1,
                        height: 1,
                        hotspot_x: 0,
                        hotspot_y: 0,
                    }),
                },
                bytes: vec![0, 0, 255, 255],
            })
        })
    }
}
impl pab_remote_desktop::DesktopSource for FixtureSource {
    fn next_frame(
        &mut self,
    ) -> pab_remote_desktop::MediaFuture<'_, pab_remote_desktop::MediaFrame> {
        // Transport fixture only, not a claim that these bytes are H.264.
        Box::pin(async {
            tokio::task::yield_now().await;
            Ok(pab_remote_desktop::MediaFrame {
                bytes: vec![0x5a; 4097],
                capture_time_us: 0,
                keyframe: true,
            })
        })
    }
}
impl pab_remote_desktop::DesktopBackend for FixtureBackend {
    fn supports_cursor(&self) -> bool {
        true
    }
    fn cursor(
        &self,
        config: pab_protocol::DesktopVideoConfig,
    ) -> pab_remote_desktop::MediaFuture<'_, Box<dyn pab_remote_desktop::DesktopCursorSource>> {
        Box::pin(async move {
            Ok(Box::new(FixtureCursor(Some(config)))
                as Box<dyn pab_remote_desktop::DesktopCursorSource>)
        })
    }
    fn screens(&self) -> pab_remote_desktop::MediaFuture<'_, Vec<pab_protocol::DesktopScreen>> {
        Box::pin(async {
            Ok(vec![pab_protocol::DesktopScreen {
                id: "fixture-primary".into(),
                name: "Fixture".into(),
                x: 0,
                y: 0,
                desktop_width: 1920,
                desktop_height: 1080,
                pixel_width: 1920,
                pixel_height: 1080,
                scale_milli: 1000,
                rotation: pab_protocol::DesktopRotation::Identity,
                primary: true,
            }])
        })
    }
    fn start(
        &self,
        _: pab_protocol::DesktopVideoConfig,
    ) -> pab_remote_desktop::MediaFuture<'_, Box<dyn pab_remote_desktop::DesktopSource>> {
        Box::pin(async {
            Ok(Box::new(FixtureSource) as Box<dyn pab_remote_desktop::DesktopSource>)
        })
    }
}

#[tokio::test]
async fn remote_desktop_backend_streams_through_task_service_without_changing_legacy_requests() {
    let temp = tempfile::tempdir().unwrap();
    let svc = service(temp.path())
        .await
        .with_remote_desktop_backend(Arc::new(FixtureBackend))
        .for_ui_connection();
    let (_a, _b, client, server) = pair().await;
    let serving = tokio::spawn(async move {
        let _guard = svc.remote_desktop_connection_guard();
        loop {
            let stream = match server.accept_bi(WAIT).await {
                Ok(stream) => stream,
                Err(_) => break,
            };
            let svc = svc.clone();
            tokio::spawn(async move {
                let _ = svc.handle_stream(actor(), stream, WAIT).await;
            });
        }
    });
    let mut env = client.open_bi(WAIT).await.unwrap();
    env.send_frame_json(
        &DeviceTaskRequest::GetEnvironment {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
        },
        WAIT,
    )
    .await
    .unwrap();
    assert!(matches!(
        env.receive_json(WAIT).await.unwrap(),
        DeviceTaskResponse::Environment {
            remote_desktop_schema_version: Some(pab_protocol::REMOTE_DESKTOP_SCHEMA_VERSION),
            remote_desktop_cursor_schema_version: Some(pab_protocol::DESKTOP_CURSOR_SCHEMA_VERSION),
            terminal_schema_version: Some(2),
            ..
        }
    ));
    let mut viewer = pab_remote_desktop::RemoteDesktopSession::open(client.clone(), None, 15, WAIT)
        .await
        .unwrap()
        .with_cursor_support(Some(pab_protocol::DESKTOP_CURSOR_SCHEMA_VERSION));
    let frame = viewer.next_frame(WAIT).await.unwrap();
    assert_eq!(frame.bytes, vec![0x5a; 4097]);
    let mut cursor = viewer.open_cursor().await.unwrap();
    let image = cursor.next_update().await.unwrap();
    assert_eq!(image.bytes, [0, 0, 255, 255]);
    assert_eq!(image.header.generation, frame.header.generation);
    let mut legacy = client.open_bi(WAIT).await.unwrap();
    legacy
        .send_frame_json(
            &DeviceTaskRequest::GetPresence {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
            },
            WAIT,
        )
        .await
        .unwrap();
    assert!(matches!(
        legacy.receive_json(WAIT).await.unwrap(),
        DeviceTaskResponse::Presence {
            active_operators: 0
        }
    ));
    viewer.report_presented(&frame.header).unwrap();
    viewer.close().await.unwrap();
    assert!(!cursor.is_current());
    serving.abort();
    let _ = serving.await;
}
