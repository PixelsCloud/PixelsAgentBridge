use super::*;
use crate::task_service::{
    filesystem_hash::HashTestGate,
    transfer_tests::{actor, pair, service},
};
use pab_protocol::{DEVICE_TASK_SCHEMA_VERSION, DeviceTaskRequest, FileSearchMode, TextEncoding};
use std::sync::Arc;

fn request(path: &Path, operation: FileSystemAction) -> FileSystemRequest {
    FileSystemRequest {
        request_id: RequestId::new(),
        path: path.to_str().unwrap().to_owned(),
        operation,
        payload_size: 0,
        payload_sha256: None,
    }
}

fn search(mode: FileSearchMode, query: &str) -> FileSystemAction {
    FileSystemAction::Search {
        mode,
        query: query.to_owned(),
        glob: "**/*".to_owned(),
        case_sensitive: true,
        max_results: 100,
        max_depth: 16,
        max_file_bytes: 4 * 1024 * 1024,
    }
}

async fn execute(service: &TaskService, request: &FileSystemRequest) -> FileSystemReply {
    service
        .execute_filesystem(actor(), request, &[])
        .await
        .unwrap()
        .0
}

async fn terminal(service: &TaskService, id: RequestId) -> FileSystemReply {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let reply = service.lookup_filesystem(actor(), id).await.unwrap();
            if !matches!(reply.state.as_str(), "running" | "cancel_requested") {
                return reply;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn name_glob_case_depth_hidden_files_and_invalid_root() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    fs::create_dir_all(work.join("src/deep")).await.unwrap();
    for name in [
        "Foo.TXT",
        ".hidden-Foo.txt",
        "src/Foo.txt",
        "src/deep/Foo.txt",
    ] {
        fs::write(work.join(name), b"text").await.unwrap();
    }
    let svc = service(dir.path()).await;
    let reply = execute(&svc, &request(&work, search(FileSearchMode::Name, "Foo"))).await;
    assert_eq!(reply.search.unwrap().matches.len(), 4);
    let mut action = search(FileSearchMode::Name, "foo");
    if let FileSystemAction::Search {
        case_sensitive,
        glob,
        ..
    } = &mut action
    {
        *case_sensitive = false;
        *glob = "src/*.txt".to_owned();
    }
    let reply = execute(&svc, &request(&work, action)).await;
    let summary = reply.search.unwrap();
    assert_eq!(summary.matches.len(), 1);
    assert!(summary.matches[0].path.ends_with("Foo.txt"));
    let mut action = search(FileSearchMode::Name, "Foo");
    if let FileSystemAction::Search { max_depth, .. } = &mut action {
        *max_depth = 1;
    }
    let reply = execute(&svc, &request(&work, action)).await;
    assert!(reply.search.as_ref().unwrap().truncated);
    assert_eq!(reply.search.unwrap().matches.len(), 2);
    let mut action = search(FileSearchMode::Name, "Foo");
    if let FileSystemAction::Search { glob, .. } = &mut action {
        *glob = "[".to_owned();
    }
    assert_eq!(
        execute(&svc, &request(&work, action))
            .await
            .error
            .unwrap()
            .code,
        "invalid_glob"
    );
    assert_eq!(
        execute(
            &svc,
            &request(&work.join("Foo.TXT"), search(FileSearchMode::Name, "Foo"))
        )
        .await
        .error
        .unwrap()
        .code,
        "not_directory"
    );
}

#[tokio::test]
async fn content_search_preserves_line_numbers_hash_and_reports_skips() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    fs::create_dir(&work).await.unwrap();
    let body = "nothing\r\nHELLO 中文\rhello 中文\n";
    let bytes = text::encode(body, TextEncoding::Utf16Le, true).unwrap();
    fs::write(work.join("中文.txt"), &bytes).await.unwrap();
    fs::write(work.join("binary.bin"), [0, 1, 2]).await.unwrap();
    fs::write(work.join("big.txt"), vec![b'a'; 200])
        .await
        .unwrap();
    let svc = service(dir.path()).await;
    let mut action = search(FileSearchMode::Content, "hello");
    if let FileSystemAction::Search {
        case_sensitive,
        max_file_bytes,
        ..
    } = &mut action
    {
        *case_sensitive = false;
        *max_file_bytes = 100;
    }
    let reply = execute(&svc, &request(&work, action)).await;
    let summary = reply.search.unwrap();
    assert_eq!(
        summary
            .matches
            .iter()
            .map(|m| m.line.unwrap())
            .collect::<Vec<_>>(),
        [2, 3]
    );
    assert!(
        summary
            .matches
            .iter()
            .all(|m| m.sha256 == Some(digest(&bytes)))
    );
    assert_eq!(summary.skipped_entries, 2);
    assert!(
        summary
            .warnings
            .iter()
            .any(|w| w.contains("file_size_limit"))
    );
    assert!(!summary.truncated);
    assert_eq!(summary.scanned_bytes, bytes.len() as u64 + 3);
}

#[tokio::test]
async fn search_match_and_escaped_json_output_budgets_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    fs::create_dir(&work).await.unwrap();
    fs::write(work.join("many.txt"), "match\n".repeat(110))
        .await
        .unwrap();
    let svc = service(dir.path()).await;
    let mut action = search(FileSearchMode::Content, "match");
    if let FileSystemAction::Search { max_results, .. } = &mut action {
        *max_results = 1;
    }
    let summary = execute(&svc, &request(&work, action)).await.search.unwrap();
    assert_eq!(summary.matches.len(), 1);
    assert_eq!(summary.stop_reason.as_deref(), Some("match_limit"));
    let content = format!("match{}\n", "\t".repeat(155));
    fs::write(work.join("many.txt"), content.repeat(100))
        .await
        .unwrap();
    let reply = execute(
        &svc,
        &request(&work, search(FileSearchMode::Content, "match")),
    )
    .await;
    assert!(serde_json::to_vec(&reply).unwrap().len() < 32 * 1024);
    let summary = reply.search.unwrap();
    assert!(summary.matches.len() < 100);
    assert_eq!(summary.stop_reason.as_deref(), Some("output_bytes_limit"));
}

#[tokio::test]
async fn directory_entry_budget_does_not_grow_with_an_entire_tree() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    fs::create_dir(&work).await.unwrap();
    for i in 0..4097 {
        std::fs::write(work.join(format!("{i}.txt")), []).unwrap();
    }
    let svc = service(dir.path()).await;
    let summary = execute(
        &svc,
        &request(&work, search(FileSearchMode::Name, "absent")),
    )
    .await
    .search
    .unwrap();
    assert!(summary.truncated);
    assert!(summary.scanned_entries <= 4096);
    assert!(matches!(
        summary.stop_reason.as_deref(),
        Some("entry_limit" | "time_limit")
    ));
}

#[tokio::test]
async fn mkdir_defaults_dedup_and_partial_creation_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let target = dir.path().join("parent/中文");
    assert_eq!(
        execute(
            &svc,
            &request(
                &target,
                FileSystemAction::Mkdir {
                    parents: false,
                    exist_ok: false
                }
            )
        )
        .await
        .error
        .unwrap()
        .code,
        "missing_parent"
    );
    assert!(!target.parent().unwrap().exists());
    let req = request(
        &target,
        FileSystemAction::Mkdir {
            parents: true,
            exist_ok: false,
        },
    );
    let reply = execute(&svc, &req).await;
    assert_eq!(reply.state, "completed");
    assert_eq!(reply.created_paths.as_ref().unwrap().len(), 2);
    fs::remove_dir(&target).await.unwrap();
    assert_eq!(execute(&svc, &req).await, reply);
    assert!(!target.exists()); // No mutation replay.
    let req = request(
        target.parent().unwrap(),
        FileSystemAction::Mkdir {
            parents: false,
            exist_ok: false,
        },
    );
    assert_eq!(
        execute(&svc, &req).await.error.unwrap().code,
        "already_exists"
    );
    let req = request(
        target.parent().unwrap(),
        FileSystemAction::Mkdir {
            parents: false,
            exist_ok: true,
        },
    );
    assert!(execute(&svc, &req).await.created_paths.unwrap().is_empty());
    let partial = dir.path().join("partial").join("x".repeat(256));
    let req = request(
        &partial,
        FileSystemAction::Mkdir {
            parents: true,
            exist_ok: false,
        },
    );
    let reply = execute(&svc, &req).await;
    assert_eq!(reply.state, "failed");
    assert!(partial.parent().unwrap().is_dir());
    assert_eq!(
        reply.created_paths.unwrap(),
        vec![partial.parent().unwrap().to_str().unwrap()]
    );
}

#[tokio::test]
async fn mkdir_rejects_files_path_lock_and_unproven_restart_does_not_replay() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let path = dir.path().join("occupied");
    fs::write(&path, b"original").await.unwrap();
    assert_eq!(
        execute(
            &svc,
            &request(
                &path,
                FileSystemAction::Mkdir {
                    parents: true,
                    exist_ok: true
                }
            )
        )
        .await
        .error
        .unwrap()
        .code,
        "not_directory"
    );
    let target = dir.path().join("locked");
    let guard = svc
        .upload_locks
        .try_acquire(&target)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        execute(
            &svc,
            &request(
                &target,
                FileSystemAction::Mkdir {
                    parents: false,
                    exist_ok: false
                }
            )
        )
        .await
        .error
        .unwrap()
        .code,
        "path_busy"
    );
    drop(guard);
    let req = request(
        &target,
        FileSystemAction::Mkdir {
            parents: false,
            exist_ok: false,
        },
    );
    let fingerprint = digest(&serde_json::to_vec(&req).unwrap());
    svc.store
        .accept_filesystem(actor(), &req, &fingerprint)
        .await
        .unwrap();
    svc.store.interrupt_read_operations().await.unwrap();
    assert_eq!(execute(&svc, &req).await.state, "unconfirmed");
    assert!(!target.exists());
}

#[tokio::test]
async fn streaming_hash_large_binary_empty_files_and_idempotent_historical_result() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let path = dir.path().join("large.bin");
    let bytes = vec![0xc3; 5 * 1024 * 1024 + 7];
    fs::write(&path, &bytes).await.unwrap();
    let req = request(&path, FileSystemAction::Hash);
    assert_eq!(execute(&svc, &req).await.state, "running");
    let reply = terminal(&svc, req.request_id).await;
    assert_eq!(reply.state, "completed");
    assert_eq!(
        reply.metadata.as_ref().unwrap().sha256,
        Some(digest(&bytes))
    );
    assert_eq!(reply.progress.unwrap().completed_bytes, bytes.len() as u64);
    fs::write(&path, b"changed").await.unwrap();
    assert_eq!(
        execute(&svc, &req).await.metadata.unwrap().sha256,
        Some(digest(&bytes))
    );
    fs::write(&path, []).await.unwrap();
    let req = request(&path, FileSystemAction::Hash);
    execute(&svc, &req).await;
    assert_eq!(
        terminal(&svc, req.request_id)
            .await
            .metadata
            .unwrap()
            .sha256,
        Some(digest(b""))
    );
    let req = request(dir.path(), FileSystemAction::Hash);
    execute(&svc, &req).await;
    assert_eq!(
        terminal(&svc, req.request_id).await.error.unwrap().code,
        "not_regular_file"
    );
}

#[tokio::test]
async fn hash_cancel_is_intent_until_worker_stops_and_foreign_actor_cannot_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let path = dir.path().join("hash.bin");
    fs::write(&path, b"content").await.unwrap();
    let gate = Arc::new(HashTestGate::default());
    *svc.hash_test_gate.lock().await = Some(gate.clone());
    let req = request(&path, FileSystemAction::Hash);
    execute(&svc, &req).await;
    gate.started.notified().await;
    let other = OperatorRef::guest(pab_protocol::EndpointKey::new([99; 32]));
    assert!(svc.cancel_hash(other, req.request_id).await.is_err());
    for _ in 0..2 {
        let reply = svc.cancel_hash(actor(), req.request_id).await.unwrap();
        assert!(matches!(
            reply.state.as_str(),
            "cancel_requested" | "cancelled"
        ));
    }
    // Cancellation must interrupt the parked task without releasing its test gate.
    let reply = terminal(&svc, req.request_id).await;
    assert_eq!(reply.state, "cancelled");
    assert!(reply.metadata.is_none());
    assert_eq!(
        svc.cancel_hash(actor(), req.request_id)
            .await
            .unwrap()
            .state,
        "cancelled"
    );
    assert_eq!(fs::read(&path).await.unwrap(), b"content");
}

#[tokio::test]
async fn hash_quota_and_restart_stop_without_automatic_reexecution() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let path = dir.path().join("hash.bin");
    fs::write(&path, b"content").await.unwrap();
    let permits = svc.hash_slots.clone().acquire_many_owned(4).await.unwrap();
    let req = request(&path, FileSystemAction::Hash);
    assert_eq!(
        execute(&svc, &req).await.error.unwrap().code,
        "executor_busy"
    );
    drop(permits);
    assert_eq!(execute(&svc, &req).await.state, "failed");
    let pending = request(&path, FileSystemAction::Hash);
    svc.store
        .accept_filesystem(
            actor(),
            &pending,
            &digest(&serde_json::to_vec(&pending).unwrap()),
        )
        .await
        .unwrap();
    svc.store
        .request_filesystem_cancel(&FileSystemReply::pending(&pending))
        .await
        .unwrap();
    assert_eq!(
        svc.lookup_filesystem(actor(), pending.request_id)
            .await
            .unwrap()
            .state,
        "unconfirmed"
    );
    svc.store.interrupt_read_operations().await.unwrap();
    assert_eq!(execute(&svc, &pending).await.state, "interrupted");
}

#[tokio::test]
async fn quic_hash_acceptance_query_and_cancel_use_independent_streams() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let path = dir.path().join("wire.bin");
    fs::write(&path, b"content").await.unwrap();
    let gate = Arc::new(HashTestGate::default());
    *svc.hash_test_gate.lock().await = Some(gate.clone());
    let req = request(&path, FileSystemAction::Hash);
    let (a, b, client, server) = pair().await;
    for (request, expected) in [
        (
            DeviceTaskRequest::FileSystem {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request: req.clone(),
            },
            "running",
        ),
        (
            DeviceTaskRequest::GetFileSystem {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id: req.request_id,
            },
            "running",
        ),
        (
            DeviceTaskRequest::CancelFileSystem {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id: req.request_id,
            },
            "cancel_requested",
        ),
    ] {
        let worker_service = svc.clone();
        let server = server.clone();
        let worker = tokio::spawn(async move {
            worker_service
                .handle_stream(
                    actor(),
                    server.accept_bi(Duration::from_secs(5)).await.unwrap(),
                    Duration::from_secs(5),
                )
                .await
                .unwrap();
        });
        let mut stream = client.open_bi(Duration::from_secs(5)).await.unwrap();
        stream
            .send_json(&request, Duration::from_secs(5))
            .await
            .unwrap();
        let reply: DeviceTaskResponse = stream.receive_json(Duration::from_secs(5)).await.unwrap();
        assert!(
            matches!(reply, DeviceTaskResponse::FileSystem { reply } if reply.state == expected || (expected == "cancel_requested" && reply.state == "cancelled"))
        );
        stream
            .expect_receive_end(Duration::from_secs(5))
            .await
            .unwrap();
        worker.await.unwrap();
    }
    gate.resume.notify_one();
    assert_eq!(terminal(&svc, req.request_id).await.state, "cancelled");
    a.close().await;
    b.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn links_cannot_escape_search_mkdir_or_hash_roots() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let root = dir.path().join("root");
    fs::create_dir(&root).await.unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).await.unwrap();
    fs::write(outside.join("secret.txt"), b"secret")
        .await
        .unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    let summary = execute(
        &svc,
        &request(&root, search(FileSearchMode::Name, "secret")),
    )
    .await
    .search
    .unwrap();
    assert!(summary.matches.is_empty());
    assert_eq!(summary.skipped_entries, 1);
    let hash = request(&root.join("link/secret.txt"), FileSystemAction::Hash);
    execute(&svc, &hash).await;
    assert_eq!(
        terminal(&svc, hash.request_id).await.error.unwrap().code,
        "link_not_supported"
    );
    let mkdir = request(
        &root.join("link/new"),
        FileSystemAction::Mkdir {
            parents: true,
            exist_ok: true,
        },
    );
    assert_eq!(
        execute(&svc, &mkdir).await.error.unwrap().code,
        "link_not_supported"
    );
    assert!(!outside.join("new").exists());
}

#[tokio::test]
async fn hash_rejects_observed_version_change_and_late_cancel_cannot_erase_success() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let path = dir.path().join("changed.bin");
    fs::write(&path, b"original content").await.unwrap();
    let gate = Arc::new(HashTestGate {
        after_open: true,
        ..Default::default()
    });
    *svc.hash_test_gate.lock().await = Some(gate.clone());
    let req = request(&path, FileSystemAction::Hash);
    execute(&svc, &req).await;
    gate.started.notified().await;
    assert_eq!(
        svc.lookup_filesystem(actor(), req.request_id)
            .await
            .unwrap()
            .progress
            .unwrap()
            .total_bytes,
        16
    );
    fs::write(&path, b"short").await.unwrap();
    gate.resume.notify_one();
    let reply = terminal(&svc, req.request_id).await;
    assert_eq!(reply.state, "failed");
    assert_eq!(reply.error.unwrap().code, "version_conflict");
    assert!(reply.metadata.unwrap().sha256.is_none());
    *svc.hash_test_gate.lock().await = None;
    let req = request(&path, FileSystemAction::Hash);
    execute(&svc, &req).await;
    let completed = terminal(&svc, req.request_id).await;
    assert_eq!(
        svc.cancel_hash(actor(), req.request_id).await.unwrap(),
        completed
    );
    svc.store
        .update_filesystem_progress(&FileSystemReply::pending(&req))
        .await
        .unwrap();
    assert_eq!(
        svc.lookup_filesystem(actor(), req.request_id)
            .await
            .unwrap(),
        completed
    );
}
