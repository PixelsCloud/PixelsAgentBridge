use super::*;
use std::collections::BTreeMap;

#[derive(Clone)]
struct File {
    bytes: Vec<u8>,
    identity: u64,
    revision: u64,
}
#[derive(Default)]
struct State {
    files: BTreeMap<String, File>,
    snapshots: usize,
    delay_ms: u64,
    denied: bool,
}
fn meta(path: &str, file: &File) -> Value {
    json!({"name":path.rsplit('/').next().unwrap(),"directory":false,"size":file.bytes.len(),"readonly":false,"identity":file.identity.to_string(),"modified_at_unix_ms":file.revision})
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
struct Fixture {
    _directory: tempfile::TempDir,
    service: Arc<FileService>,
    state: Arc<std::sync::Mutex<State>>,
    pump: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.pump.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let service = FileService::open(
            &directory.path().join("file.sqlite"),
            directory.path(),
            [6; 32],
        )
        .await
        .unwrap();
        let state = Arc::new(std::sync::Mutex::new(State::default()));
        let worker = service.clone();
        let shared = state.clone();
        let pump = tokio::spawn(async move {
            loop {
                let authorization = worker.authorization.poll().await;
                if !authorization.is_null() {
                    let denied = shared.lock().unwrap().denied;
                    worker
                        .authorization
                        .complete(
                            serde_json::from_value(authorization["ticket"].clone()).unwrap(),
                            if denied {
                                json!({"error":"control_paused"})
                            } else {
                                json!({})
                            },
                        )
                        .await;
                }
                let job = worker.storage.poll().await;
                if !job.is_null() {
                    let worker = worker.clone();
                    let shared = shared.clone();
                    tokio::spawn(async move {
                        let command = &job["command"];
                        let path = command["path"].as_str().unwrap();
                        let action = command["action"].as_str().unwrap();
                        let delay = if action == "snapshot" {
                            let mut state = shared.lock().unwrap();
                            state.snapshots += 1;
                            state.delay_ms
                        } else {
                            0
                        };
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        let response = {
                            let mut state = shared.lock().unwrap();
                            if state.denied {
                                json!({"error":"control_paused"})
                            } else {
                                match action {
                                    "stat" if path == "/workspace" => {
                                        json!({"metadata":{"name":"workspace","directory":true}})
                                    }
                                    "stat" => state
                                        .files
                                        .get(path)
                                        .map(|file| json!({"metadata":meta(path,file)}))
                                        .unwrap_or(json!({"error":"not_found"})),
                                    "snapshot" => {
                                        if let Some(file) = state.files.get(path) {
                                            let stage = worker.staging.join(format!(
                                                "{}.part",
                                                command["staging_id"].as_str().unwrap()
                                            ));
                                            std::fs::write(stage, &file.bytes).unwrap();
                                            json!({"metadata":meta(path,file)})
                                        } else {
                                            json!({"error":"not_found"})
                                        }
                                    }
                                    "publish" => {
                                        let stage = worker.staging.join(format!(
                                            "{}.part",
                                            command["staging_id"].as_str().unwrap()
                                        ));
                                        let bytes = std::fs::read(stage).unwrap();
                                        if state.files.get(path).is_some_and(|old| {
                                            command["expected_hash"]
                                                .as_str()
                                                .is_some_and(|hash| hash != digest(&old.bytes))
                                        }) {
                                            json!({"error":"hash_conflict"})
                                        } else {
                                            let file = File {
                                                bytes,
                                                identity: 42,
                                                revision: 42,
                                            };
                                            let result = json!({"metadata":meta(path,&file)});
                                            state.files.insert(path.into(), file);
                                            result
                                        }
                                    }
                                    "list" => {
                                        let after = command["after"].as_str().unwrap_or("");
                                        let entries = state
                                            .files
                                            .iter()
                                            .filter(|(key, _)| key.starts_with(&format!("{path}/")))
                                            .filter(|(key, _)| !key[path.len() + 1..].contains('/'))
                                            .map(|(key, file)| meta(key, file))
                                            .filter(|m| m["name"].as_str().unwrap() > after)
                                            .collect::<Vec<_>>();
                                        json!({"entries":entries,"has_more":false})
                                    }
                                    "log_probe" => {
                                        if let Some(file) = state.files.get(path) {
                                            let bytes = &file.bytes;
                                            let bom = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
                                                3
                                            } else if bytes.starts_with(&[0xff, 0xfe])
                                                || bytes.starts_with(&[0xfe, 0xff])
                                            {
                                                2
                                            } else {
                                                0
                                            };
                                            let mut start = command["offset"]
                                                .as_u64()
                                                .map(|x| x as usize)
                                                .unwrap_or_else(|| {
                                                    command["tail_bytes"].as_u64().map_or(0, |n| {
                                                        bytes.len().saturating_sub(n as usize)
                                                    })
                                                })
                                                .max(bom);
                                            if !command["tail_bytes"].is_null()
                                                && bom == 2
                                                && !(start - bom).is_multiple_of(2)
                                            {
                                                start += 1;
                                            }
                                            if start > bytes.len() {
                                                json!({"error":"invalid_range"})
                                            } else {
                                                let context = start.saturating_sub(1024);
                                                let end = (start
                                                    + command["max_bytes"].as_u64().unwrap()
                                                        as usize)
                                                    .min(bytes.len());
                                                json!({"start":start,"context_start":context,"window":hex(&bytes[context..end]),"head":hex(&bytes[..bytes.len().min(1024)]),"identity":file.identity.to_string(),"metadata":meta(path,file)})
                                            }
                                        } else {
                                            json!({"error":"not_found"})
                                        }
                                    }
                                    _ => panic!("Unexpected test SDK action: {action}"),
                                }
                            }
                        };
                        worker
                            .storage
                            .complete(
                                serde_json::from_value(job["ticket"].clone()).unwrap(),
                                response,
                            )
                            .await;
                    });
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        });
        Self {
            _directory: directory,
            service,
            state,
            pump,
        }
    }
    fn put(&self, path: &str, bytes: Vec<u8>) {
        self.state.lock().unwrap().files.insert(
            path.into(),
            File {
                bytes,
                identity: 1,
                revision: 1,
            },
        );
    }
}
fn request(path: &str, operation: FileSystemAction, payload: &[u8]) -> FileSystemRequest {
    FileSystemRequest {
        execution: ExecutionSelection::default(),
        request_id: RequestId::new(),
        path: path.into(),
        payload_size: payload.len() as u32,
        payload_sha256: operation.has_payload().then(|| digest(payload)),
        operation,
    }
}
fn read(path: &str, cursor: Option<String>, encoding: Option<TextEncoding>) -> FileSystemRequest {
    request(
        path,
        FileSystemAction::Read {
            range: TextReadRange::Stream {
                cursor,
                offset: None,
                tail_bytes: None,
                max_bytes: 4,
                wait_ms: 0,
            },
            encoding,
            expected_hash: None,
        },
        &[],
    )
}

#[tokio::test]
async fn streaming_unicode_cursors_owner_append_replacement_and_truncation() {
    let fixture = Fixture::new().await;
    let service = &fixture.service;
    let path = "/workspace/log";
    for encoding in [
        TextEncoding::Utf8,
        TextEncoding::Utf8Bom,
        TextEncoding::Utf16Le,
        TextEncoding::Utf16Be,
    ] {
        fixture.put(path, encode("甲😀乙", encoding, true).unwrap());
        let mut cursor = None;
        let mut actual = Vec::new();
        for _ in 0..20 {
            let (reply, data) = service
                .request("owner", read(path, cursor, Some(encoding)), vec![])
                .await;
            assert_eq!(reply.state, "completed", "{reply:?}");
            actual.extend(data);
            cursor = Some(reply.log.unwrap().cursor);
            if !reply.range.unwrap().truncated {
                break;
            }
        }
        assert_eq!(String::from_utf8(actual).unwrap(), "甲😀乙");
        let (foreign, _) = service
            .request("other", read(path, cursor.clone(), Some(encoding)), vec![])
            .await;
        assert_eq!(foreign.error.unwrap().code, "invalid_cursor");
        fixture
            .state
            .lock()
            .unwrap()
            .files
            .get_mut(path)
            .unwrap()
            .bytes
            .extend(
                encode(
                    "新",
                    if encoding == TextEncoding::Utf8Bom {
                        TextEncoding::Utf8
                    } else {
                        encoding
                    },
                    false,
                )
                .unwrap(),
            );
        let (reply, data) = service
            .request("owner", read(path, cursor.clone(), Some(encoding)), vec![])
            .await;
        assert_eq!(reply.state, "completed", "{reply:?}");
        assert_eq!(String::from_utf8(data).unwrap(), "新");
        cursor = Some(reply.log.unwrap().cursor);
        fixture
            .state
            .lock()
            .unwrap()
            .files
            .get_mut(path)
            .unwrap()
            .identity += 1;
        let (reply, _) = service
            .request("owner", read(path, cursor.clone(), Some(encoding)), vec![])
            .await;
        assert_eq!(reply.error.unwrap().code, "log_changed");
        fixture
            .state
            .lock()
            .unwrap()
            .files
            .get_mut(path)
            .unwrap()
            .bytes
            .clear();
        let (reply, _) = service
            .request("owner", read(path, cursor, Some(encoding)), vec![])
            .await;
        assert_eq!(reply.error.unwrap().code, "log_changed");
    }
}

#[tokio::test]
async fn delayed_read_original_result_survives_dropped_waiter_pause_and_expires_without_replay() {
    let fixture = Fixture::new().await;
    fixture.put("/workspace/a.txt", "原始正文😀".as_bytes().to_vec());
    fixture.state.lock().unwrap().delay_ms = 80;
    let req = request(
        "/workspace/a.txt",
        FileSystemAction::Read {
            range: TextReadRange::Lines {
                start_line: 1,
                count: 10,
            },
            encoding: None,
            expected_hash: None,
        },
        &[],
    );
    let id = req.request_id;
    let task = tokio::spawn({
        let service = fixture.service.clone();
        async move { service.request("owner", req, vec![]).await }
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while fixture.state.lock().unwrap().snapshots == 0 {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    task.abort();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while fixture.service.lookup("owner", id).await.state == "running" {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let (reply, data) = fixture.service.result("owner", id).await;
    assert_eq!(reply.state, "completed");
    assert_eq!(data, "原始正文😀".as_bytes());
    fixture.state.lock().unwrap().denied = true;
    let (reply, data) = fixture.service.result("owner", id).await;
    assert_eq!(reply.error.unwrap().code, "control_paused");
    assert!(data.is_empty());
    assert_eq!(fixture.service.lookup("owner", id).await.state, "completed");
    fixture.state.lock().unwrap().denied = false;
    fixture
        .service
        .results
        .lock()
        .await
        .get_mut(&("owner".into(), id))
        .unwrap()
        .created = tokio::time::Instant::now() - Duration::from_secs(901);
    let (reply, _) = fixture.service.result("owner", id).await;
    assert_eq!(reply.error.unwrap().code, "result_expired");
    assert_eq!(fixture.state.lock().unwrap().snapshots, 1);
}

#[tokio::test]
async fn dry_run_and_apply_have_exact_same_bytes_and_hash_conflicts_preserve_external_changes() {
    let fixture = Fixture::new().await;
    let path = "/workspace/a.txt";
    let original = encode("甲 old 😀", TextEncoding::Utf16Le, true).unwrap();
    fixture.put(path, original.clone());
    let payload = serde_json::to_vec(&vec![TextEdit {
        find: "old".into(),
        replace: "新".into(),
        expected_matches: 1,
    }])
    .unwrap();
    let preview = request(
        path,
        FileSystemAction::Patch {
            dry_run: true,
            expected_hash: digest(&original),
            encoding: None,
        },
        &payload,
    );
    let (reply, _) = fixture
        .service
        .request("owner", preview, payload.clone())
        .await;
    assert_eq!(reply.state, "completed", "{reply:?}");
    let plan = reply.patch_preview.unwrap();
    assert_eq!(fixture.state.lock().unwrap().files[path].bytes, original);
    let actual = request(
        path,
        FileSystemAction::Patch {
            dry_run: false,
            expected_hash: digest(&original),
            encoding: None,
        },
        &payload,
    );
    let (reply, _) = fixture
        .service
        .request("owner", actual, payload.clone())
        .await;
    assert_eq!(reply.state, "completed", "{reply:?}");
    assert_eq!(
        digest(&fixture.state.lock().unwrap().files[path].bytes),
        plan.result_sha256
    );
    let conflict = request(
        path,
        FileSystemAction::Patch {
            dry_run: false,
            expected_hash: digest(&original),
            encoding: None,
        },
        &payload,
    );
    let (reply, _) = fixture.service.request("owner", conflict, payload).await;
    assert_eq!(reply.error.unwrap().code, "hash_conflict");
}

#[tokio::test]
async fn search_pagination_authentication_query_and_inventory_changes_are_explicit() {
    let fixture = Fixture::new().await;
    fixture.put(
        "/workspace/a.txt",
        "头\n错误1\n尾\n错误2".as_bytes().to_vec(),
    );
    let operation = FileSystemAction::Search {
        mode: FileSearchMode::Content,
        query: "错误[12]".into(),
        glob: "**".into(),
        case_sensitive: true,
        max_results: 1,
        max_depth: 4,
        max_file_bytes: 1024,
        options: FileSearchOptions {
            regex: true,
            context_lines: 1,
            ..Default::default()
        },
    };
    let (reply, _) = fixture
        .service
        .request(
            "owner",
            request("/workspace", operation.clone(), &[]),
            vec![],
        )
        .await;
    assert_eq!(reply.state, "completed", "{reply:?}");
    let summary = reply.search.unwrap();
    assert_eq!(summary.matches[0].line, Some(2));
    assert_eq!(summary.matches[0].context.len(), 3);
    let mut next = operation;
    let token = summary.next_cursor.unwrap();
    if let FileSystemAction::Search { options, .. } = &mut next {
        options.cursor = Some(token);
    }
    let (foreign, _) = fixture
        .service
        .request("other", request("/workspace", next.clone(), &[]), vec![])
        .await;
    assert_eq!(foreign.error.unwrap().code, "invalid_cursor");
    let (reply, _) = fixture
        .service
        .request("owner", request("/workspace", next.clone(), &[]), vec![])
        .await;
    assert_eq!(reply.state, "completed", "{reply:?}");
    assert_eq!(reply.search.unwrap().matches[0].line, Some(4));
    fixture
        .state
        .lock()
        .unwrap()
        .files
        .get_mut("/workspace/a.txt")
        .unwrap()
        .revision += 1;
    let (reply, _) = fixture
        .service
        .request("owner", request("/workspace", next, &[]), vec![])
        .await;
    assert_eq!(reply.error.unwrap().code, "search_changed");
}

#[tokio::test]
async fn saturated_admission_is_bounded_and_cancelled_slots_are_reusable() {
    let fixture = Fixture::new().await;
    fixture.put("/workspace/a.txt", b"one".to_vec());
    fixture.state.lock().unwrap().delay_ms = 5000;
    let make = || {
        request(
            "/workspace/a.txt",
            FileSystemAction::Read {
                range: TextReadRange::Lines {
                    start_line: 1,
                    count: 10,
                },
                encoding: None,
                expected_hash: None,
            },
            &[],
        )
    };
    let mut tasks = Vec::new();
    let mut ids = Vec::new();
    for _ in 0..16 {
        let request = make();
        ids.push(request.request_id);
        let service = fixture.service.clone();
        tasks.push(tokio::spawn(async move {
            service.request("owner", request, vec![]).await
        }));
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while fixture.service.jobs.lock().await.len() < 16 {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    while fixture.state.lock().unwrap().snapshots == 0 {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let excess = make();
    let id = excess.request_id;
    let (rejected, _) = fixture.service.request("owner", excess, vec![]).await;
    assert_eq!(rejected.error.unwrap().code, "storage_busy");
    assert_eq!(
        fixture
            .service
            .lookup("owner", id)
            .await
            .error
            .unwrap()
            .code,
        "not_found"
    );
    for id in ids {
        fixture.service.cancel("owner", id).await;
    }
    for task in tasks {
        task.await.unwrap();
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(7);
    while !fixture.service.jobs.lock().await.is_empty() {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(fixture.state.lock().unwrap().snapshots, 1);
    fixture.state.lock().unwrap().delay_ms = 0;
    let (reply, body) = fixture.service.request("owner", make(), vec![]).await;
    assert_eq!(reply.state, "completed", "{reply:?}");
    assert_eq!(body, b"one");
}

#[tokio::test]
async fn queued_read_cancellation_never_dispatches_it_and_live_read_result_is_discarded() {
    let fixture = Fixture::new().await;
    fixture.put("/workspace/a.txt", b"one".to_vec());
    fixture.state.lock().unwrap().delay_ms = 1000;
    let make = || {
        request(
            "/workspace/a.txt",
            FileSystemAction::Read {
                range: TextReadRange::Lines {
                    start_line: 1,
                    count: 10,
                },
                encoding: None,
                expected_hash: None,
            },
            &[],
        )
    };
    let first = make();
    let first_id = first.request_id;
    let first_task = tokio::spawn({
        let service = fixture.service.clone();
        async move { service.request("owner", first, vec![]).await }
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let count = fixture.state.lock().unwrap().snapshots;
        if count == 1 {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let second = make();
    let second_id = second.request_id;
    let second_task = tokio::spawn({
        let service = fixture.service.clone();
        async move { service.request("owner", second, vec![]).await }
    });
    while fixture.service.jobs.lock().await.len() < 2 {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(
        fixture.service.cancel("owner", second_id).await.state,
        "cancel_requested"
    );
    assert_eq!(
        fixture.service.cancel("owner", first_id).await.state,
        "cancel_requested"
    );
    first_task.await.unwrap();
    second_task.await.unwrap();
    for id in [first_id, second_id] {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let reply = fixture.service.lookup("owner", id).await;
            if reply.state == "cancelled" {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "{reply:?}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    assert_eq!(fixture.state.lock().unwrap().snapshots, 1);
}
