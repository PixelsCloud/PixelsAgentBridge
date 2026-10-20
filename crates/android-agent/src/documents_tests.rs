use super::*;

async fn next(bridge: &StorageBridge) -> Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let job = bridge.poll().await;
            if !job.is_null() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap()
}
async fn complete(bridge: &StorageBridge, job: &Value, value: Value) {
    assert!(
        bridge
            .complete(
                serde_json::from_value(job["ticket"].clone()).unwrap(),
                value
            )
            .await
    );
}
fn open_request() -> DocumentRequest {
    DocumentRequest {
        request_id: RequestId::new(),
        operation: DocumentAction::Open {
            path: "/workspace/文档.txt".into(),
            execution: ExecutionSelection::Service {},
            ocr_mode: DocumentOcrMode::Off,
        },
    }
}
async fn parsed(service: &DocumentService, input: &[u8]) {
    let auth = next(&service.sdk).await;
    assert_eq!(auth["command"]["action"], "authorize");
    complete(&service.sdk, &auth, json!({})).await;
    let snapshot = next(&service.files.storage).await;
    assert_eq!(snapshot["command"]["action"], "snapshot");
    let id = snapshot["command"]["staging_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let stage = service.files.stage(id);
    std::fs::write(&stage.0, input).unwrap();
    // The SDK provider owns the file until native acceptance of the snapshot.
    std::mem::forget(stage);
    complete(
        &service.files.storage,
        &snapshot,
        json!({"size":input.len(),"sha256":crate::filesystem::digest(input)}),
    )
    .await;
    let parse = next(&service.sdk).await;
    assert_eq!(parse["command"]["action"], "extract");
    let directory = Path::new(parse["command"]["directory"].as_str().unwrap());
    let document =
        pab_document::extract_snapshot(&directory.join("input"), "txt", pab_document::OcrMode::Off)
            .await
            .unwrap();
    std::fs::write(
        directory.join("result.json"),
        serde_json::to_vec(&document).unwrap(),
    )
    .unwrap();
    complete(&service.sdk, &parse, json!({"ready":true})).await;
}
async fn finished(service: &DocumentService, owner: &str, id: RequestId) -> DocumentReply {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let reply = service.lookup(owner, id).await;
            if reply.state != "running" {
                return reply;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn snapshot_owner_dedup_restart_and_permission_revocation_do_not_reparse() {
    let temp = tempfile::tempdir().unwrap();
    let staging = temp.path().join("staging");
    std::fs::create_dir(&staging).unwrap();
    let files = FileService::open(&temp.path().join("files.sqlite"), &staging, [5; 32])
        .await
        .unwrap();
    let database = temp.path().join("mobile.documents.sqlite");
    let service = DocumentService::open(&database, [5; 32], files.clone())
        .await
        .unwrap();
    let request = open_request();
    let id = request.request_id;
    assert_eq!(
        service.request("owner", request.clone()).await.state,
        "running"
    );
    parsed(&service, "正文中文 immutable snapshot".as_bytes()).await;
    let outcome = finished(&service, "owner", id).await;
    assert_eq!(outcome.state, "completed", "{outcome:?}");
    let worker = service.clone();
    let repeated = request.clone();
    let replay = tokio::spawn(async move { worker.request("owner", repeated).await });
    let auth = next(&service.sdk).await;
    complete(&service.sdk, &auth, json!({})).await;
    assert_eq!(replay.await.unwrap().state, "completed");
    assert!(service.sdk.poll().await.is_null());
    assert_eq!(
        service.lookup("foreign", id).await.error.as_deref(),
        Some("document_expired_or_not_found")
    );
    let mut conflict = request.clone();
    if let DocumentAction::Open { path, .. } = &mut conflict.operation {
        *path = "/workspace/different.txt".into();
    }
    assert_eq!(
        service.request("owner", conflict).await.error.as_deref(),
        Some("request_conflict")
    );
    let worker = service.clone();
    let read = tokio::spawn(async move {
        worker
            .request(
                "owner",
                DocumentRequest {
                    request_id: RequestId::new(),
                    operation: DocumentAction::Read {
                        document_ref: id,
                        selector: DocumentSelector::Chunk { number: 1 },
                        cursor: None,
                    },
                },
            )
            .await
    });
    let auth = next(&service.sdk).await;
    complete(&service.sdk, &auth, json!({"error":"mount_not_found"})).await;
    assert_eq!(
        read.await.unwrap().error.as_deref(),
        Some("mount_not_found")
    );
    assert!(service.files.storage.poll().await.is_null());
    let worker = service.clone();
    let revoked = tokio::spawn(async move { worker.authorized_lookup("owner", id).await });
    let auth = next(&service.sdk).await;
    complete(&service.sdk, &auth, json!({"error":"mount_not_found"})).await;
    let denied = revoked.await.unwrap();
    assert_eq!(denied.error.as_deref(), Some("mount_not_found"));
    assert!(denied.data.is_none());
    assert_eq!(service.lookup("owner", id).await.state, "completed");
    drop(service);
    let reopened = DocumentService::open(&database, [5; 32], files)
        .await
        .unwrap();
    assert_eq!(
        reopened.request("owner", request).await.error.as_deref(),
        Some("result_expired_original_not_replayed")
    );
    assert!(reopened.sdk.poll().await.is_null());
    assert_eq!(std::fs::read_dir(&reopened.root).unwrap().count(), 0);
}

#[tokio::test]
async fn cancellation_after_dispatch_discards_parser_result_and_cleans_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let staging = temp.path().join("staging");
    std::fs::create_dir(&staging).unwrap();
    let files = FileService::open(&temp.path().join("files.sqlite"), &staging, [7; 32])
        .await
        .unwrap();
    let service =
        DocumentService::open(&temp.path().join("mobile.documents.sqlite"), [7; 32], files)
            .await
            .unwrap();
    let request = open_request();
    let id = request.request_id;
    service.request("owner", request).await;
    assert_eq!(service.cancel("foreign", id).await.state, "failed");
    let auth = next(&service.sdk).await;
    let pending = service.cancel("owner", id).await;
    assert!(matches!(
        pending.state.as_str(),
        "cancel_requested" | "cancelled"
    ));
    // Cancellation can happen before SDK dispatch; accepted work is not replayed.
    complete(&service.sdk, &auth, json!({"error":"control_paused"})).await;
    assert_eq!(finished(&service, "owner", id).await.state, "cancelled");
    assert!(service.entries.lock().await.is_empty());
    assert_eq!(std::fs::read_dir(&service.root).unwrap().count(), 0);
}
