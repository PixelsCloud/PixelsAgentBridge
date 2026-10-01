use super::super::transfer_tests::{actor, pair, service};
use super::*;
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Method, Response, StatusCode, Uri},
    response::IntoResponse,
};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;
const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
struct FixtureState {
    detail: Mutex<Value>,
    list: Mutex<Vec<Value>>,
    log: Mutex<Vec<u8>>,
    requests: Mutex<Vec<String>>,
    posts: AtomicUsize,
    post_seen: Notify,
    hold: Mutex<Option<Arc<Notify>>>,
    reject_post: Mutex<Option<u16>>,
    vanish: Mutex<bool>,
    deny_inspect: Mutex<bool>,
    engine: Mutex<String>,
}
struct Fixture {
    state: Arc<FixtureState>,
    docker: Docker,
    endpoint: String,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(held) = self.state.hold.try_lock()
            && let Some(gate) = held.as_ref()
        {
            gate.notify_waiters();
        }
        self.server.abort();
    }
}
fn detail() -> Value {
    json!({"Id":ID,"Name":"/app","Image":"sha256:fixture","Created":"2026-10-01T00:00:00Z","State":{"Status":"exited","Running":false,"Paused":false,"Restarting":false,"Dead":false,"Pid":0,"ExitCode":0,"OOMKilled":false,"Error":"","StartedAt":"old","FinishedAt":"old-end","Health":{"Status":"healthy"}},"Config":{"Image":"fixture:latest","Tty":false,"Env":["PASSWORD=not-for-results"],"Cmd":["secret-command"],"Labels":{"project":"测试"}},"HostConfig":{"RestartPolicy":{"Name":"no"},"LogConfig":{"Type":"json-file"}},"Mounts":[{"Type":"bind","Source":"/local/source","Destination":"/data","RW":false}],"NetworkSettings":{"Ports":{"8080/tcp":[{"HostIp":"127.0.0.1","HostPort":"18080"}]},"Networks":{"bridge":{"IPAddress":"172.17.0.2","Gateway":"172.17.0.1","MacAddress":"00:11:22:33:44:55"}}}})
}
fn summary(state: &str, id: &str, name: &str) -> Value {
    json!({"Id":id,"Names":[name],"Image":"fixture:latest","ImageID":"sha256:fixture","State":state,"Status":state,"Created":1,"Ports":[],"Labels":{"project":"测试"}})
}
fn response(status: u16, body: Value) -> axum::response::Response {
    (StatusCode::from_u16(status).unwrap(), axum::Json(body)).into_response()
}
async fn handler(
    State(s): State<Arc<FixtureState>>,
    method: Method,
    uri: Uri,
) -> axum::response::Response {
    s.requests.lock().await.push(format!("{method} {uri}"));
    let raw = uri.path();
    let path = if raw.starts_with("/v1.") {
        raw[1..].find('/').map(|i| &raw[i + 1..]).unwrap_or(raw)
    } else {
        raw
    };
    if path == "/version" {
        return response(
            200,
            json!({"ApiVersion":"1.41","MinAPIVersion":"1.24","Version":"fixture"}),
        );
    }
    if path == "/info" {
        return response(
            200,
            json!({"ID":s.engine.lock().await.clone(),"OSType":"linux","Architecture":"x86_64","ServerVersion":"fixture"}),
        );
    }
    if path == "/containers/json" {
        return response(200, json!(s.list.lock().await.clone()));
    }
    if path.ends_with("/json") {
        if *s.deny_inspect.lock().await {
            return response(403, json!({"message":"fixture denied"}));
        }
        if *s.vanish.lock().await {
            return response(404, json!({"message":"container disappeared"}));
        }
        return response(200, s.detail.lock().await.clone());
    }
    if path.ends_with("/logs") {
        return Response::builder()
            .header("content-type", "application/vnd.docker.raw-stream")
            .body(Body::from(s.log.lock().await.clone()))
            .unwrap();
    }
    if method == Method::POST {
        s.posts.fetch_add(1, Ordering::SeqCst);
        if let Some(status) = *s.reject_post.lock().await {
            s.post_seen.notify_one();
            return response(status, json!({"message":"fixture rejection"}));
        }
        if !path.contains(ID) {
            return response(400, json!({"message":"must use pinned full ID"}));
        }
        let mut d = s.detail.lock().await;
        let stop = path.ends_with("/stop");
        d["State"]["Status"] = json!(if stop { "exited" } else { "running" });
        d["State"]["Running"] = json!(!stop);
        if !stop {
            d["State"]["StartedAt"] = json!(format!("new-{}", s.posts.load(Ordering::SeqCst)));
        }
        drop(d);
        s.post_seen.notify_one();
        let hold = s.hold.lock().await.clone();
        if let Some(hold) = hold {
            hold.notified().await;
        }
        return StatusCode::NO_CONTENT.into_response();
    }
    response(404, json!({"message":"fixture unknown route"}))
}
impl Fixture {
    async fn new() -> Self {
        let state = Arc::new(FixtureState {
            detail: Mutex::new(detail()),
            list: Mutex::new(vec![
                summary("running", ID, "/app"),
                summary("exited", &"b".repeat(64), "/stopped"),
            ]),
            log: Mutex::new(vec![]),
            requests: Mutex::new(vec![]),
            posts: AtomicUsize::new(0),
            post_seen: Notify::new(),
            hold: Mutex::new(None),
            reject_post: Mutex::new(None),
            vanish: Mutex::new(false),
            deny_inspect: Mutex::new(false),
            engine: Mutex::new(RequestId::new().to_string()),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().fallback(handler).with_state(state.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let docker = Docker::connect_with_http(&endpoint, 30, API_DEFAULT_VERSION).unwrap();
        Self {
            state,
            docker,
            endpoint,
            server,
        }
    }
    fn q(&self, action: ContainerAction) -> ContainerQuery {
        ContainerQuery {
            action,
            timeout_ms: 5000,
        }
    }
    async fn invoke(&self, action: ContainerAction) -> SystemQueryReply {
        let (_send, cancelled) = watch::channel(false);
        query_with_client(
            RequestId::new(),
            &self.q(action),
            cancelled,
            None,
            self.docker.clone(),
            self.endpoint.clone(),
        )
        .await
    }
    async fn ready(&self) {
        tokio::time::timeout(Duration::from_secs(3), self.state.post_seen.notified())
            .await
            .unwrap();
    }
}
fn snap(r: &SystemQueryReply) -> &ContainerSnapshot {
    let Some(SystemQueryData::Container { snapshot }) = &r.data else {
        panic!("no container snapshot {r:?}")
    };
    snapshot
}
fn list(all: bool) -> ContainerAction {
    ContainerAction::List {
        all,
        name: None,
        states: vec![],
        labels: vec![],
        limit: 100,
    }
}
fn logs() -> ContainerAction {
    ContainerAction::Logs {
        container: "app".into(),
        since: None,
        until: None,
        tail: 200,
        stdout: true,
        stderr: true,
        timestamps: true,
        max_bytes: 16384,
    }
}
fn control(control: ContainerControl) -> ContainerAction {
    ContainerAction::Control {
        container: "app".into(),
        control,
        stop_timeout_seconds: 7,
    }
}
fn frame(stream: u8, bytes: &[u8]) -> Vec<u8> {
    let mut b = vec![stream, 0, 0, 0];
    b.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    b.extend_from_slice(bytes);
    b
}
#[tokio::test]
async fn inventory_details_filters_and_negotiation_are_structured_and_private() {
    let f = Fixture::new().await;
    let r = f.invoke(list(false)).await;
    assert_eq!(r.state, "completed", "{r:?}");
    assert_eq!(snap(&r).entries.len(), 1);
    assert_eq!(snap(&r).engine_os.as_deref(), Some("linux"));
    assert_eq!(snap(&r).api_version.as_deref(), Some("1.41"));
    let r = f
        .invoke(ContainerAction::List {
            all: true,
            name: Some("STOP".into()),
            states: vec![],
            labels: vec!["project=测试".into()],
            limit: 1,
        })
        .await;
    assert_eq!(snap(&r).entries[0].names[0], "/stopped");
    let r = f
        .invoke(ContainerAction::Get {
            container: "app".into(),
        })
        .await;
    assert_eq!(r.state, "completed", "{r:?}");
    let c = snap(&r).container.as_ref().unwrap();
    assert_eq!(c.ports[0].public_port, Some(18080));
    assert_eq!(c.state.health.as_deref(), Some("healthy"));
    assert_eq!(c.mounts[0].destination.as_deref(), Some("/data"));
    let saved = serde_json::to_string(&r).unwrap();
    assert!(!saved.contains("not-for-results"));
    assert!(!saved.contains("secret-command"));
    assert!(
        f.state
            .requests
            .lock()
            .await
            .iter()
            .any(|r| r.contains("/v1.41/containers"))
    );
    let r = f
        .invoke(ContainerAction::Get {
            container: "aaaa".into(),
        })
        .await;
    assert_eq!(r.state, "failed");
    assert!(r.error.unwrap().contains("abbreviated"));
}
#[tokio::test]
async fn log_frames_keep_streams_unicode_and_explicit_byte_truncation() {
    let f = Fixture::new().await;
    let utf = "你好🙂\n".as_bytes();
    let mut data = frame(1, &utf[..2]);
    data.extend(frame(2, b"error\n"));
    data.extend(frame(1, &utf[2..]));
    *f.state.log.lock().await = data;
    let r = f.invoke(logs()).await;
    assert_eq!(r.state, "completed", "{r:?}");
    assert_eq!(snap(&r).logs[0].text, "你好🙂\n");
    assert_eq!(snap(&r).logs[1].stream, "stderr");
    assert!(!r.warnings.iter().any(|w| w.contains("replacement")));
    *f.state.log.lock().await = frame(1, &vec![b'X'; 20000]);
    let r = f.invoke(logs()).await;
    assert!(r.truncated);
    assert!(snap(&r).logs[0].text.len() <= 16384);
    *f.state.log.lock().await = frame(2, &[0xff, b'\n']);
    let r = f.invoke(logs()).await;
    assert!(r.warnings.iter().any(|w| w.contains("replacement")));
    let req = f.state.requests.lock().await;
    let req = req.iter().find(|r| r.contains("/logs")).unwrap();
    assert!(req.contains("follow=false"));
    assert!(req.contains(ID));
}
#[tokio::test]
async fn tty_logs_are_merged_and_stderr_only_is_rejected() {
    let f = Fixture::new().await;
    f.state.detail.lock().await["Config"]["Tty"] = json!(true);
    *f.state.log.lock().await = b"console output\n".to_vec();
    let r = f.invoke(logs()).await;
    assert_eq!(r.state, "completed", "{r:?}");
    assert_eq!(snap(&r).logs[0].stream, "console");
    let mut q = logs();
    if let ContainerAction::Logs { stdout, .. } = &mut q {
        *stdout = false;
    }
    let r = f.invoke(q).await;
    assert_eq!(r.state, "failed");
    assert!(r.error.unwrap().contains("tty_streams_merged"));
}
#[tokio::test]
async fn start_stop_restart_pin_id_and_confirm_real_state() {
    let f = Fixture::new().await;
    for c in [
        ContainerControl::Start,
        ContainerControl::Restart,
        ContainerControl::Stop,
    ] {
        let r = f.invoke(control(c)).await;
        assert_eq!(r.state, "completed", "{r:?}");
        assert!(
            snap(&r).submission_started
                && snap(&r).daemon_acknowledged
                && snap(&r).desired_state_observed
        );
        assert_eq!(snap(&r).container_id.as_deref(), Some(ID));
    }
    let r = f.invoke(control(ContainerControl::Stop)).await;
    assert_eq!(r.state, "completed");
    assert!(!snap(&r).submission_started);
    assert_eq!(f.state.posts.load(Ordering::SeqCst), 3);
    let requests = f.state.requests.lock().await;
    assert!(
        requests
            .iter()
            .filter(|r| r.starts_with("POST"))
            .all(|r| r.contains(ID))
    );
    assert!(requests.iter().any(|r| r.contains("restart?t=7")));
}
#[tokio::test]
async fn cancellation_and_busy_preserve_effects_and_reconcile_without_replay() {
    let f = Fixture::new().await;
    *f.state.hold.lock().await = Some(Arc::new(Notify::new()));
    let (send, cancelled) = watch::channel(false);
    let q = f.q(control(ContainerControl::Start));
    let docker = f.docker.clone();
    let endpoint = f.endpoint.clone();
    let worker = tokio::spawn(async move {
        query_with_client(RequestId::new(), &q, cancelled, None, docker, endpoint).await
    });
    f.ready().await;
    let busy = f.invoke(control(ContainerControl::Stop)).await;
    assert_eq!(busy.state, "failed");
    assert!(busy.error.unwrap().contains("container_busy"));
    send.send(true).unwrap();
    let r = worker.await.unwrap();
    assert_eq!(r.state, "unconfirmed", "{r:?}");
    assert!(snap(&r).submission_started);
    assert!(!snap(&r).daemon_acknowledged);
    let resolved = reconcile_client(&f.q(control(ContainerControl::Start)), &r, f.docker.clone())
        .await
        .unwrap();
    assert_eq!(resolved.state, "completed");
    assert_eq!(f.state.posts.load(Ordering::SeqCst), 1);
    *f.state.engine.lock().await = "different-engine".into();
    assert!(
        reconcile_client(&f.q(control(ContainerControl::Start)), &r, f.docker.clone())
            .await
            .is_none()
    );
}
#[tokio::test]
async fn deadline_missing_container_permissions_and_pre_cancel_are_explicit() {
    let f = Fixture::new().await;
    *f.state.hold.lock().await = Some(Arc::new(Notify::new()));
    let (_send, cancelled) = watch::channel(false);
    let mut q = f.q(control(ContainerControl::Start));
    q.timeout_ms = 600;
    let r = query_with_client(
        RequestId::new(),
        &q,
        cancelled,
        None,
        f.docker.clone(),
        f.endpoint.clone(),
    )
    .await;
    assert_eq!(r.state, "unconfirmed", "{r:?}");
    assert!(r.error.as_ref().unwrap().contains("deadline"));
    assert_eq!(f.state.posts.load(Ordering::SeqCst), 1);
    *f.state.vanish.lock().await = true;
    let r = f
        .invoke(ContainerAction::Get {
            container: "app".into(),
        })
        .await;
    assert_eq!(r.state, "failed");
    assert!(r.error.unwrap().contains("docker_api_404"));
    *f.state.vanish.lock().await = false;
    *f.state.deny_inspect.lock().await = true;
    let r = f
        .invoke(ContainerAction::Get {
            container: "app".into(),
        })
        .await;
    assert!(r.error.unwrap().contains("docker_api_403"));
    let (_send, cancelled) = watch::channel(true);
    let r = query_with_client(
        RequestId::new(),
        &f.q(control(ContainerControl::Stop)),
        cancelled,
        None,
        f.docker.clone(),
        f.endpoint.clone(),
    )
    .await;
    assert_eq!(r.state, "cancelled");
    assert!(!snap(&r).submission_started);
    assert!(local_client("tcp://unrelated:2375").is_err());
    #[cfg(windows)]
    {
        assert!(local_client("npipe:////./pipe/x").is_ok());
        assert!(local_client("npipe:////server/pipe/x").is_err());
        assert!(local_client("npipe:////./pipe/x/y").is_err());
    }
}
#[tokio::test]
async fn system_operations_deduplicate_and_cancel_only_the_owner() {
    let f = Fixture::new().await;
    let dir = tempfile::tempdir().unwrap();
    let mut svc = service(dir.path()).await;
    svc.container_client = Some((f.docker.clone(), f.endpoint.clone()));
    *f.state.hold.lock().await = Some(Arc::new(Notify::new()));
    let id = RequestId::new();
    let spec = SystemQuery::Container {
        query: f.q(control(ContainerControl::Start)),
    };
    let r = svc.system_query(actor(), id, spec.clone()).await.unwrap();
    assert_eq!(r.state, "running");
    f.ready().await;
    assert_eq!(
        svc.system_query(actor(), id, spec.clone())
            .await
            .unwrap()
            .state,
        "running"
    );
    let other = OperatorRef::guest(EndpointKey::new([99; 32]));
    assert!(svc.cancel_system_query(other, id).await.is_err());
    assert_eq!(
        svc.cancel_system_query(actor(), id).await.unwrap().state,
        "cancel_requested"
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let r = svc.store.get_system_query(actor(), id).await.unwrap();
        if r.state != "running" {
            assert_eq!(r.state, "unconfirmed");
            break;
        }
        assert!(Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
    assert_eq!(f.state.posts.load(Ordering::SeqCst), 1);
    let busy = svc
        .system_query(
            actor(),
            RequestId::new(),
            SystemQuery::Container {
                query: f.q(control(ContainerControl::Restart)),
            },
        )
        .await
        .unwrap();
    assert_eq!(busy.state, "failed", "{busy:?}");
    assert!(busy.error.unwrap().contains("busy_unconfirmed"));
    assert_eq!(f.state.posts.load(Ordering::SeqCst), 1);
    let replay = svc.system_query(actor(), id, spec.clone()).await.unwrap();
    assert_eq!(replay.state, "unconfirmed");
    let mut changed = spec.clone();
    if let SystemQuery::Container { query } = &mut changed {
        query.timeout_ms += 1;
    }
    assert!(svc.system_query(actor(), id, changed).await.is_err());
    let original = svc.store.get_system_query(actor(), id).await.unwrap();
    let resolved = reconcile_client(
        &f.q(control(ContainerControl::Start)),
        &original,
        f.docker.clone(),
    )
    .await
    .unwrap();
    assert_eq!(resolved.state, "completed");
    svc.store.finish_system_query(&resolved).await.unwrap();
    let no_op = svc
        .system_query(actor(), RequestId::new(), spec.clone())
        .await
        .unwrap();
    assert_eq!(no_op.state, "completed", "{no_op:?}");
    assert!(!snap(&no_op).submission_started);
    assert_eq!(f.state.posts.load(Ordering::SeqCst), 1);
    let second = RequestId::new();
    assert!(
        svc.store
            .accept_system_query(actor(), second, &spec)
            .await
            .unwrap()
    );
    drop(svc);
    let reopened_service = service(dir.path()).await;
    let reopened = &reopened_service.store;
    assert_eq!(
        reopened
            .get_system_query(actor(), second)
            .await
            .unwrap()
            .state,
        "unconfirmed"
    );
    assert!(
        !reopened
            .accept_system_query(actor(), second, &spec)
            .await
            .unwrap()
    );
}
#[tokio::test]
async fn serialized_results_handle_escaping_and_large_details() {
    let f = Fixture::new().await;
    f.state.detail.lock().await["Config"]["Labels"] = json!(
        (0..100)
            .map(|i| (format!("label{i}"), "\u{0001}".repeat(2000)))
            .collect::<HashMap<_, _>>()
    );
    let r = f
        .invoke(ContainerAction::Get {
            container: "app".into(),
        })
        .await;
    assert_eq!(r.state, "completed");
    assert!(r.truncated);
    assert!(serde_json::to_vec(&r).unwrap().len() <= MAX_SYSTEM_REPLY_BYTES);
    for key in ["Image", "Created"] {
        f.state.detail.lock().await[key] = json!("\u{0001}".repeat(1000));
    }
    for key in ["Image"] {
        f.state.detail.lock().await["Config"][key] = json!("\u{0001}".repeat(1000));
    }
    f.state.detail.lock().await["State"]["Error"] = json!("\u{0001}".repeat(1000));
    let escaped = f
        .invoke(ContainerAction::Get {
            container: "app".into(),
        })
        .await;
    assert_eq!(escaped.state, "completed", "{escaped:?}");
    assert!(serde_json::to_vec(&escaped).unwrap().len() <= MAX_SYSTEM_REPLY_BYTES);
    assert_eq!(snap(&escaped).container_id.as_deref(), Some(ID));
    f.state.detail.lock().await["State"]["StartedAt"] = json!("x".repeat(300));
    let invalid = f.invoke(control(ContainerControl::Restart)).await;
    assert_eq!(invalid.state, "failed");
    assert_eq!(f.state.posts.load(Ordering::SeqCst), 0);
    f.state.detail.lock().await["State"]["StartedAt"] = json!("old");
    *f.state.log.lock().await = frame(1, &vec![1; 17000]);
    let r = f.invoke(logs()).await;
    assert!(r.truncated);
    assert!(serde_json::to_vec(&r).unwrap().len() <= MAX_SYSTEM_REPLY_BYTES);
}
#[tokio::test]
async fn container_result_and_capability_cross_real_quic() {
    let f = Fixture::new().await;
    let dir = tempfile::tempdir().unwrap();
    let mut svc = service(dir.path()).await;
    svc.container_client = Some((f.docker.clone(), f.endpoint.clone()));
    let (a, b, client, server) = pair().await;
    let id = RequestId::new();
    let mut original = None;
    for req in [
        DeviceTaskRequest::GetEnvironment {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
        },
        DeviceTaskRequest::SystemQuery {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
            request_id: id,
            query: SystemQuery::Container {
                query: f.q(ContainerAction::Get {
                    container: "app".into(),
                }),
            },
        },
        DeviceTaskRequest::GetSystemQuery {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
            request_id: id,
        },
    ] {
        let svc = svc.clone();
        let server = server.clone();
        let worker = tokio::spawn(async move {
            svc.handle_stream(
                actor(),
                server.accept_bi(Duration::from_secs(5)).await.unwrap(),
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        });
        let mut stream = client.open_bi(Duration::from_secs(5)).await.unwrap();
        stream
            .send_json(&req, Duration::from_secs(5))
            .await
            .unwrap();
        let r: DeviceTaskResponse = stream.receive_json(Duration::from_secs(5)).await.unwrap();
        match r {
            DeviceTaskResponse::Environment {
                system_query_schema_version,
                ..
            } => assert_eq!(system_query_schema_version, Some(6)),
            DeviceTaskResponse::SystemQuery { reply } => {
                assert_eq!(reply.state, "completed");
                if let Some(prior) = &original {
                    assert_eq!(&reply, prior);
                } else {
                    original = Some(reply);
                    f.state.detail.lock().await["State"]["Status"] = json!("changed-after-sample");
                }
            }
            other => panic!("{other:?}"),
        };
        stream
            .expect_receive_end(Duration::from_secs(5))
            .await
            .unwrap();
        worker.await.unwrap();
    }
    a.close().await;
    b.close().await;
}

#[tokio::test]
async fn rejected_controls_disappearance_and_wrong_framing_are_not_successes() {
    let f = Fixture::new().await;
    *f.state.reject_post.lock().await = Some(403);
    let r = f.invoke(control(ContainerControl::Start)).await;
    assert_eq!(r.state, "failed", "{r:?}");
    assert!(snap(&r).submission_started);
    assert!(!snap(&r).daemon_acknowledged);
    assert!(r.error.unwrap().contains("docker_api_403"));
    *f.state.reject_post.lock().await = Some(500);
    let r = f.invoke(control(ContainerControl::Start)).await;
    assert_eq!(r.state, "unconfirmed");
    *f.state.vanish.lock().await = true;
    assert!(
        reconcile_client(&f.q(control(ContainerControl::Start)), &r, f.docker.clone())
            .await
            .is_none()
    );
    *f.state.vanish.lock().await = false;
    f.state.detail.lock().await["Id"] = json!("b".repeat(64));
    let mismatch = f
        .invoke(ContainerAction::Get {
            container: ID.into(),
        })
        .await;
    assert!(mismatch.error.unwrap().contains("identity_mismatch"));
    f.state.detail.lock().await["Id"] = json!(ID);
    *f.state.log.lock().await = b"invalid unframed data".to_vec();
    let r = f.invoke(logs()).await;
    assert_eq!(r.state, "failed");
    assert!(r.error.unwrap().contains("invalid_log_framing"));
}
#[tokio::test]
async fn replaced_name_never_redirects_a_pinned_control() {
    let f = Fixture::new().await;
    *f.state.hold.lock().await = Some(Arc::new(Notify::new()));
    let (send, cancelled) = watch::channel(false);
    let q = f.q(control(ContainerControl::Start));
    let docker = f.docker.clone();
    let endpoint = f.endpoint.clone();
    let worker = tokio::spawn(async move {
        query_with_client(RequestId::new(), &q, cancelled, None, docker, endpoint).await
    });
    f.ready().await;
    send.send(true).unwrap();
    let original = worker.await.unwrap();
    // The old container disappears. A replacement with the same name is never inspected by name.
    *f.state.vanish.lock().await = true;
    let result = reconcile_client(
        &f.q(control(ContainerControl::Start)),
        &original,
        f.docker.clone(),
    )
    .await;
    assert!(result.is_none());
    let requests = f.state.requests.lock().await;
    let last = requests.iter().rev().find(|r| r.contains("/json")).unwrap();
    assert!(last.contains(ID));
    assert_eq!(f.state.posts.load(Ordering::SeqCst), 1);
}
struct OwnedContainer(String);
impl Drop for OwnedContainer {
    fn drop(&mut self) {
        let mut c = std::process::Command::new("docker");
        c.args(["rm", "--force", &self.0]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x08000000);
        }
        let _ = c.output();
    }
}
#[tokio::test]
#[ignore = "requires local Docker and existing debian:bookworm-slim image; creates/removes only one labeled test container"]
async fn native_docker_named_pipe_controls_inspection_and_logs() {
    let name = format!("pab-e2-{}", RequestId::new());
    let mut command = std::process::Command::new("docker");
    command.args(["create","--name",&name,"--label","pab.test=agent-work-tools-e2","--network","none","--memory","64m","--cpus","0.25","debian:bookworm-slim","sh","-c","printf 'pixel stdout 中文\n'; printf 'pixel stderr\n' >&2; trap 'exit 0' TERM; while :; do sleep 1; done"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let id = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    assert!(oid(&id));
    let _owned = OwnedContainer(id.clone());
    let invoke = |action| {
        let q = ContainerQuery {
            action,
            timeout_ms: 30000,
        };
        async move {
            let (_send, cancelled) = watch::channel(false);
            query(RequestId::new(), &q, cancelled, None).await
        }
    };
    let r = invoke(ContainerAction::Get {
        container: name.clone(),
    })
    .await;
    assert_eq!(r.state, "completed", "{r:?}");
    assert_eq!(snap(&r).container_id.as_deref(), Some(id.as_str()));
    assert_eq!(
        snap(&r).container.as_ref().unwrap().state.status.as_deref(),
        Some("created")
    );
    let r = invoke(ContainerAction::Control {
        container: name.clone(),
        control: ContainerControl::Start,
        stop_timeout_seconds: 3,
    })
    .await;
    assert_eq!(r.state, "completed", "{r:?}");
    assert!(snap(&r).daemon_acknowledged && snap(&r).desired_state_observed);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let r = invoke(ContainerAction::Logs {
            container: id.clone(),
            since: None,
            until: None,
            tail: 20,
            stdout: true,
            stderr: true,
            timestamps: true,
            max_bytes: 16384,
        })
        .await;
        assert_eq!(r.state, "completed", "{r:?}");
        if snap(&r)
            .logs
            .iter()
            .any(|l| l.stream == "stdout" && l.text.contains("pixel stdout 中文"))
            && snap(&r)
                .logs
                .iter()
                .any(|l| l.stream == "stderr" && l.text.contains("pixel stderr"))
        {
            break;
        }
        assert!(Instant::now() < until, "logs missing {r:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let r = invoke(ContainerAction::List {
        all: true,
        name: Some(name.clone()),
        states: vec!["running".into()],
        labels: vec!["pab.test=agent-work-tools-e2".into()],
        limit: 10,
    })
    .await;
    assert_eq!(r.state, "completed", "{r:?}");
    assert!(snap(&r).entries.iter().any(|s| s.id == id));
    for c in [ContainerControl::Restart, ContainerControl::Stop] {
        let r = invoke(ContainerAction::Control {
            container: id.clone(),
            control: c,
            stop_timeout_seconds: 3,
        })
        .await;
        assert_eq!(r.state, "completed", "{r:?}");
        assert!(snap(&r).desired_state_observed);
    }
    let r = invoke(ContainerAction::Get {
        container: id.clone(),
    })
    .await;
    assert_eq!(
        snap(&r).container.as_ref().unwrap().state.status.as_deref(),
        Some("exited")
    );
    println!(
        "native Docker verified endpoint={}, API={}, engine={}, temporary_container={}",
        snap(&r).endpoint,
        snap(&r).api_version.as_deref().unwrap_or(""),
        snap(&r).engine_id.as_deref().unwrap_or(""),
        id
    );
}
