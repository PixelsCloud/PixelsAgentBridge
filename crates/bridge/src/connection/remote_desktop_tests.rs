// Real loopback QUIC and production DesktopHost/Client. These are transport
// fixtures, not native-video or operating-system login acceptance.
use pab_protocol::*;
use pab_remote_desktop::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct ChangingBackend {
    started: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
}
struct ChangingSource(Arc<AtomicUsize>);
impl Drop for ChangingSource {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl DesktopSource for ChangingSource {
    fn next_frame(&mut self) -> MediaFuture<'_, MediaFrame> {
        Box::pin(async {
            Ok(MediaFrame {
                bytes: vec![7; 16],
                capture_time_us: 0,
                keyframe: true,
            })
        })
    }
}
impl DesktopBackend for ChangingBackend {
    fn screens(&self) -> MediaFuture<'_, Vec<DesktopScreen>> {
        Box::pin(async {
            Ok(vec![DesktopScreen {
                id: "fixture".into(),
                name: "Fixture".into(),
                x: 0,
                y: 0,
                desktop_width: 640,
                desktop_height: 480,
                pixel_width: 640,
                pixel_height: 480,
                scale_milli: 1000,
                rotation: DesktopRotation::Identity,
                primary: true,
            }])
        })
    }
    fn start(&self, _: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopSource>> {
        Box::pin(async {
            self.started.store(true, Ordering::SeqCst);
            self.active.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(ChangingSource(self.active.clone())) as Box<dyn DesktopSource>)
        })
    }
}

async fn changing_cursor_capability(before: Option<u16>, after: Option<u16>, broken_refresh: bool) {
    const WAIT: Duration = Duration::from_secs(5);
    let (_a, _b, client, server) = test_connection_pair().await;
    let device = client.device_ref();
    let owner = client.operator();
    let started = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicUsize::new(0));
    let queries = Arc::new(AtomicUsize::new(0));
    let host = DesktopHost::new(Some(Arc::new(ChangingBackend {
        started: started.clone(),
        active: active.clone(),
    })))
    .for_connection();
    let seen = queries.clone();
    let serving = tokio::spawn(async move {
        let mut streams = tokio::task::JoinSet::new();
        loop {
            let mut stream = server.accept_bi(WAIT).await.unwrap();
            let request = stream
                .receive_json::<DeviceTaskRequest>(WAIT)
                .await
                .unwrap();
            match request {
                DeviceTaskRequest::GetEnvironment { .. } => {
                    seen.fetch_add(1, Ordering::SeqCst);
                    let ready = started.load(Ordering::SeqCst);
                    if ready && broken_refresh {
                        stream
                            .send_json(&serde_json::json!({"type":"invalid_fixture_reply"}), WAIT)
                            .await
                            .unwrap();
                        continue;
                    }
                    let context = serde_json::json!({"device_ref":device,"execution":{"os_family":"linux","os_name":"Linux","os_version":"fixture","architecture":"x86_64","execution_scope":"native","path_style":"posix","interpreter":null,"cwd":"/fixture","environment_revision":"fixture"},"source":"executor_verified","observed_at_unix_ms":1,"freshness":"current"});
                    let mut value = serde_json::json!({"type":"environment","context":context,"remote_desktop_schema_version":REMOTE_DESKTOP_SCHEMA_VERSION});
                    // Omit absent capability as an older executor would.
                    if let Some(version) = if ready { after } else { before } {
                        value["remote_desktop_cursor_schema_version"] = version.into();
                    }
                    stream.send_json(&value, WAIT).await.unwrap();
                }
                DeviceTaskRequest::RemoteDesktop { request, .. } => {
                    let host = host.clone();
                    streams
                        .spawn(async move { host.serve(owner, None, request, stream, WAIT).await });
                }
                DeviceTaskRequest::GetPresence { .. } => {
                    stream
                        .send_json(
                            &DeviceTaskResponse::Presence {
                                active_operators: 1,
                            },
                            WAIT,
                        )
                        .await
                        .unwrap();
                }
                _ => panic!("unexpected fixture request"),
            }
        }
    });
    let mut viewer = client.open_remote_desktop(None, 15).await.unwrap();
    let supported = viewer.supports_cursor();
    let query_count = queries.load(Ordering::SeqCst);
    assert_eq!(viewer.next_frame(WAIT).await.unwrap().bytes, vec![7; 16]);
    // Capability refresh must not replace or break the existing task connection.
    assert!(matches!(
        client
            .task_request(DeviceTaskRequest::GetPresence {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
            })
            .await
            .unwrap(),
        DeviceTaskResponse::Presence {
            active_operators: 1
        }
    ));
    viewer.close().await.unwrap();
    assert_eq!(active.load(Ordering::SeqCst), 0);
    serving.abort();
    let _ = serving.await;
    assert_eq!(
        supported,
        !broken_refresh && after == Some(DESKTOP_CURSOR_SCHEMA_VERSION)
    );
    assert_eq!(query_count, 2, "refresh only after the capture has started");
}

#[tokio::test]
async fn cursor_capability_is_refreshed_after_capture_startup() {
    changing_cursor_capability(None, Some(DESKTOP_CURSOR_SCHEMA_VERSION), false).await;
}

#[tokio::test]
async fn stale_cursor_capability_is_not_reused_for_legacy_or_unknown_helper() {
    for after in [None, Some(999)] {
        changing_cursor_capability(Some(DESKTOP_CURSOR_SCHEMA_VERSION), after, false).await;
    }
}

#[tokio::test]
async fn optional_cursor_refresh_failure_keeps_video_and_legacy_tasks_usable() {
    changing_cursor_capability(Some(DESKTOP_CURSOR_SCHEMA_VERSION), None, true).await;
}
