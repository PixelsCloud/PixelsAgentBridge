use super::*;

fn write_request(id: RequestId, text: &[u8]) -> FileSystemRequest {
    FileSystemRequest {
        execution: ExecutionSelection::default(),
        request_id: id,
        path: "/workspace/中文.txt".into(),
        operation: FileSystemAction::Write {
            encoding: TextEncoding::Utf8,
            overwrite: false,
            expected_hash: None,
        },
        payload_size: text.len() as u32,
        payload_sha256: Some(digest(text)),
    }
}

async fn finish_publish(service: &FileService) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let job = service.storage.poll().await;
        if !job.is_null() {
            assert_eq!(job["command"]["action"], "publish");
            let stage = service.staging.join(format!(
                "{}.part",
                job["command"]["staging_id"].as_str().unwrap()
            ));
            assert_eq!(std::fs::read(stage).unwrap(), "共享文本".as_bytes());
            service
                .storage
                .complete(
                    serde_json::from_value(job["ticket"].clone()).unwrap(),
                    json!({"metadata":{"directory":false,"size":12,"readonly":false}}),
                )
                .await;
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Platform job was not submitted"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn accepted_write_is_durable_and_does_not_republish_after_reconnect_or_restart() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("files.sqlite");
    let service = FileService::open(&database, directory.path(), [3; 32])
        .await
        .unwrap();
    let payload = "共享文本".as_bytes().to_vec();
    let request = write_request(RequestId::new(), &payload);
    let worker = service.clone();
    let original = request.clone();
    let bytes = payload.clone();
    let pending = tokio::spawn(async move { worker.request("first", original, bytes).await });
    finish_publish(&service).await;
    assert_eq!(pending.await.unwrap().0.state, "completed");
    let (again, _) = service
        .request("first", request.clone(), payload.clone())
        .await;
    assert_eq!(again.state, "completed");
    assert!(service.storage.poll().await.is_null());
    let foreign = service.lookup("other", request.request_id).await;
    assert_eq!(foreign.error.unwrap().code, "not_found");
    drop(service);
    let reopened = FileService::open(&database, directory.path(), [3; 32])
        .await
        .unwrap();
    let (again, _) = reopened.request("first", request.clone(), payload).await;
    assert_eq!(again.state, "completed");
    assert!(reopened.storage.poll().await.is_null());
    let different = b"different".to_vec();
    let (conflict, _) = reopened
        .request(
            "first",
            write_request(request.request_id, &different),
            different,
        )
        .await;
    assert_eq!(conflict.error.unwrap().code, "request_conflict");
    assert!(reopened.storage.poll().await.is_null());
}

#[tokio::test]
async fn process_interruption_preserves_uncertainty_without_reexecuting_the_write() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("files.sqlite");
    let service = FileService::open(&database, directory.path(), [4; 32])
        .await
        .unwrap();
    let payload = b"unknown".to_vec();
    let request = write_request(RequestId::new(), &payload);
    let key = blake3::keyed_hash(&[4; 32], &serde_json::to_vec(&request).unwrap())
        .to_hex()
        .to_string();
    sqlx::query("INSERT INTO file_operations(owner,id,digest,reply) VALUES(?,?,?,?)")
        .bind("first")
        .bind(request.request_id.to_string())
        .bind(key)
        .bind(serde_json::to_string(&FileSystemReply::pending(&request)).unwrap())
        .execute(&service.pool)
        .await
        .unwrap();
    drop(service);
    let reopened = FileService::open(&database, directory.path(), [4; 32])
        .await
        .unwrap();
    let (reply, _) = reopened.request("first", request, payload).await;
    assert_eq!(reply.state, "unconfirmed");
    assert_eq!(reply.error.unwrap().code, "process_interrupted");
    assert!(reopened.storage.poll().await.is_null());
}

#[tokio::test]
async fn payload_integrity_failure_is_rejected_before_platform_execution() {
    let directory = tempfile::tempdir().unwrap();
    let service = FileService::open(
        &directory.path().join("files.sqlite"),
        directory.path(),
        [4; 32],
    )
    .await
    .unwrap();
    let request = write_request(RequestId::new(), b"original");
    let (reply, _) = service
        .request("first", request, b"tampered".to_vec())
        .await;
    assert_eq!(reply.error.unwrap().code, "invalid_payload");
    assert!(service.storage.poll().await.is_null());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM file_operations")
        .fetch_one(&service.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn cancelled_mailbox_waiter_is_not_dispatched() {
    let bridge = StorageBridge::new();
    let child = bridge.clone();
    let task = tokio::spawn(async move {
        child
            .call(json!({"action":"delete","path":"/workspace/test"}))
            .await
    });
    for _ in 0..100 {
        if !bridge.pending_is_empty().await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    task.abort();
    let _ = task.await;
    assert!(bridge.poll().await.is_null());
}

#[tokio::test]
async fn hash_cancellation_waits_for_the_worker_and_is_isolated_by_caller() {
    let directory = tempfile::tempdir().unwrap();
    let service = FileService::open(
        &directory.path().join("files.sqlite"),
        directory.path(),
        [7; 32],
    )
    .await
    .unwrap();
    let guard = service.execution.lock().await;
    let request = FileSystemRequest {
        execution: Default::default(),
        request_id: RequestId::new(),
        path: "/workspace/large.bin".into(),
        operation: FileSystemAction::Hash,
        payload_size: 0,
        payload_sha256: None,
    };
    let (accepted, _) = service.request("first", request.clone(), Vec::new()).await;
    assert_eq!(accepted.state, "running");
    assert_eq!(
        service
            .cancel("other", request.request_id)
            .await
            .error
            .unwrap()
            .code,
        "not_found"
    );
    assert_eq!(
        service.cancel("first", request.request_id).await.state,
        "cancel_requested"
    );
    drop(guard);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let reply = service.lookup("first", request.request_id).await;
        if reply.state == "cancelled" {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(service.storage.poll().await.is_null());
    assert_eq!(
        service.request("first", request, Vec::new()).await.0.state,
        "cancelled"
    );
}

#[tokio::test]
async fn journal_failure_is_uncertain_instead_of_claiming_an_operation_does_not_exist() {
    let directory = tempfile::tempdir().unwrap();
    let service = FileService::open(
        &directory.path().join("files.sqlite"),
        directory.path(),
        [7; 32],
    )
    .await
    .unwrap();
    service.pool.close().await;
    let reply = service.lookup("first", RequestId::new()).await;
    assert_eq!(reply.state, "unconfirmed");
    assert_eq!(reply.error.unwrap().code, "journal_unavailable");
}

#[tokio::test]
async fn provider_response_loss_preserves_uncertainty_and_does_not_retry_a_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("files.sqlite");
    let service = FileService::open(&database, directory.path(), [3; 32])
        .await
        .unwrap();
    let request = write_request(RequestId::new(), "共享文本".as_bytes());
    let worker = service.clone();
    let original = request.clone();
    let pending = tokio::spawn(async move {
        worker
            .request("owner", original, "共享文本".as_bytes().to_vec())
            .await
    });
    let job = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let value = service.storage.poll().await;
            if !value.is_null() {
                break value;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(job["command"]["action"], "publish");
    // The provider applied its side effect, then lost the RPC response.
    let target = directory.path().join("provider-side-effect.txt");
    std::fs::write(&target, "共享文本").unwrap();
    service
        .storage
        .complete(
            serde_json::from_value(job["ticket"].clone()).unwrap(),
            json!({"error":"publication_unconfirmed"}),
        )
        .await;
    let (reply, _) = pending.await.unwrap();
    assert_eq!(reply.state, "unconfirmed");
    assert_eq!(
        service
            .request("owner", request.clone(), "共享文本".as_bytes().to_vec())
            .await
            .0
            .state,
        "unconfirmed"
    );
    assert!(service.storage.poll().await.is_null());
    service.pool.close().await;
    drop(service);
    let restarted = FileService::open(&database, directory.path(), [3; 32])
        .await
        .unwrap();
    assert_eq!(
        restarted
            .request("owner", request, "共享文本".as_bytes().to_vec())
            .await
            .0
            .state,
        "unconfirmed"
    );
    assert_eq!(std::fs::read_to_string(target).unwrap(), "共享文本");
    assert!(restarted.storage.poll().await.is_null());
}
