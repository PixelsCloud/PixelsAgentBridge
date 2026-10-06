use super::*;
use crate::task_service::{
    filesystem_bulk::BulkTestGate,
    transfer_tests::{actor, pair, service},
};
use pab_protocol::{DEVICE_TASK_SCHEMA_VERSION, DeviceTaskRequest, FileOperationLimits};
use std::{
    io::{Cursor, Write},
    sync::Arc,
};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

fn request(path: &Path, operation: FileSystemAction) -> FileSystemRequest {
    FileSystemRequest {
        execution: Default::default(),
        request_id: RequestId::new(),
        path: path.to_str().unwrap().to_owned(),
        operation,
        payload_size: 0,
        payload_sha256: None,
    }
}
fn copy(dest: &Path, recursive: bool, overwrite: bool) -> FileSystemAction {
    FileSystemAction::Copy {
        destination: dest.to_str().unwrap().to_owned(),
        recursive,
        overwrite,
        limits: FileOperationLimits::default(),
    }
}
fn moving(dest: &Path) -> FileSystemAction {
    FileSystemAction::Move {
        destination: dest.to_str().unwrap().to_owned(),
        recursive: true,
        overwrite: false,
        limits: FileOperationLimits::default(),
    }
}
fn extract(dest: &Path) -> FileSystemAction {
    FileSystemAction::ArchiveExtract {
        destination: dest.to_str().unwrap().to_owned(),
        overwrite: false,
        max_ratio: 200,
        limits: FileOperationLimits::default(),
    }
}
async fn execute(svc: &TaskService, req: &FileSystemRequest) -> FileSystemReply {
    svc.execute_filesystem(actor(), req, &[]).await.unwrap().0
}
async fn terminal(svc: &TaskService, id: RequestId) -> FileSystemReply {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let reply = svc.lookup_filesystem(actor(), id).await.unwrap();
            if matches!(reply.state.as_str(), "completed" | "failed" | "cancelled") {
                return reply;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
async fn run(svc: &TaskService, req: &FileSystemRequest) -> FileSystemReply {
    let accepted = execute(svc, req).await;
    if accepted.state == "running" {
        terminal(svc, req.request_id).await
    } else {
        accepted
    }
}
async fn gate(svc: &TaskService, phase: &'static str) -> Arc<BulkTestGate> {
    let gate = Arc::new(BulkTestGate {
        phase,
        started: Default::default(),
        resume: Default::default(),
    });
    *svc.bulk_test_gate.lock().await = Some(gate.clone());
    gate
}
fn zip_bytes(entries: &[(&str, &[u8])], compression: CompressionMethod) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, body) in entries {
        writer
            .start_file(
                *name,
                SimpleFileOptions::default().compression_method(compression),
            )
            .unwrap();
        writer.write_all(body).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[tokio::test]
async fn binary_copy_explicit_overwrite_hash_and_original_id_never_replays() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let src = dir.path().join("source.bin");
    let dst = dir.path().join("target.bin");
    let bytes = vec![0xbd; 5 * 1024 * 1024 + 17];
    fs::write(&src, &bytes).await.unwrap();
    fs::write(&dst, b"original").await.unwrap();
    let fail = run(&svc, &request(&src, copy(&dst, false, false))).await;
    assert_eq!(fail.error.unwrap().code, "already_exists");
    assert_eq!(fs::read(&dst).await.unwrap(), b"original");
    let req = request(&src, copy(&dst, false, true));
    let done = run(&svc, &req).await;
    assert_eq!(done.state, "completed", "{done:?}");
    assert_eq!(done.metadata.as_ref().unwrap().sha256, Some(digest(&bytes)));
    assert_eq!(
        done.progress.as_ref().unwrap().completed_bytes,
        bytes.len() as u64
    );
    assert_eq!(fs::read(&dst).await.unwrap(), bytes);
    assert!(src.exists());
    fs::remove_file(&dst).await.unwrap();
    assert_eq!(execute(&svc, &req).await, done);
    assert!(!dst.exists());
}

#[tokio::test]
async fn recursive_copy_preflight_merge_and_resource_limits() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let src = dir.path().join("source");
    let dst = dir.path().join("target");
    fs::create_dir_all(src.join("empty")).await.unwrap();
    fs::write(src.join("a.txt"), b"alpha").await.unwrap();
    assert_eq!(
        run(&svc, &request(&src, copy(&dst, false, false)))
            .await
            .error
            .unwrap()
            .code,
        "recursive_required"
    );
    assert!(!dst.exists());
    for (count, bytes, depth, expected) in
        [(1, 100, 64, "entry_limit"), (10, 4, 64, "bytes_limit")].map(|(c, b, d, e)| (c, b, d, e))
    {
        let mut action = copy(&dst, true, false);
        if let FileSystemAction::Copy { limits, .. } = &mut action {
            *limits = FileOperationLimits {
                max_entries: count,
                max_bytes: bytes,
                max_depth: depth,
            };
        }
        assert_eq!(
            run(&svc, &request(&src, action)).await.error.unwrap().code,
            expected
        );
        assert!(!dst.exists());
    }
    fs::create_dir(&dst).await.unwrap();
    fs::write(dst.join("unrelated"), b"keep").await.unwrap();
    assert_eq!(
        run(&svc, &request(&src, copy(&dst, true, true)))
            .await
            .state,
        "completed"
    );
    assert!(dst.join("empty").is_dir());
    assert_eq!(fs::read(dst.join("unrelated")).await.unwrap(), b"keep");
    assert_eq!(
        run(&svc, &request(&src, copy(&src.join("inside"), true, true)))
            .await
            .error
            .unwrap()
            .code,
        "overlapping_paths"
    );
}

#[tokio::test]
async fn recursive_move_verifies_target_then_removes_source() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let src = dir.path().join("source");
    let dst = dir.path().join("target");
    fs::create_dir_all(src.join("中文/empty")).await.unwrap();
    fs::write(src.join("中文/数据.bin"), [0, 255, 1])
        .await
        .unwrap();
    let done = run(&svc, &request(&src, moving(&dst))).await;
    assert_eq!(done.state, "completed", "{done:?}");
    assert!(!src.exists());
    assert_eq!(
        fs::read(dst.join("中文/数据.bin")).await.unwrap(),
        [0, 255, 1]
    );
    let summary = done.mutation.unwrap();
    assert!(summary.source_removed);
    assert!(!summary.partial);
    assert_eq!(summary.deleted_entries, 4);
}

#[tokio::test]
async fn changed_move_target_preserves_source_and_reports_partial() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let src = dir.path().join("source");
    let dst = dir.path().join("target");
    fs::write(&src, b"source").await.unwrap();
    let gate = gate(&svc, "before_source_delete").await;
    let req = request(&src, moving(&dst));
    execute(&svc, &req).await;
    gate.started.notified().await;
    fs::write(&dst, b"changed").await.unwrap();
    gate.resume.notify_one();
    let done = terminal(&svc, req.request_id).await;
    assert_eq!(done.state, "failed");
    assert!(done.mutation.as_ref().unwrap().partial);
    assert!(!done.mutation.unwrap().source_removed);
    assert_eq!(fs::read(&src).await.unwrap(), b"source");
    assert_eq!(fs::read(&dst).await.unwrap(), b"changed");
}

#[tokio::test]
async fn cooperative_cancel_waits_for_worker_and_cleans_staging_not_original() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let src = dir.path().join("source");
    let dst = dir.path().join("target");
    fs::write(&src, b"source").await.unwrap();
    fs::write(&dst, b"original").await.unwrap();
    let gate = gate(&svc, "before_publish").await;
    let req = request(&src, copy(&dst, false, true));
    execute(&svc, &req).await;
    gate.started.notified().await;
    let other = OperatorRef::guest(pab_protocol::EndpointKey::new([99; 32]));
    assert!(svc.cancel_hash(other, req.request_id).await.is_err());
    assert_eq!(
        svc.cancel_hash(actor(), req.request_id)
            .await
            .unwrap()
            .state,
        "cancel_requested"
    );
    assert_eq!(
        svc.lookup_filesystem(actor(), req.request_id)
            .await
            .unwrap()
            .state,
        "cancel_requested"
    );
    gate.resume.notify_one();
    let done = terminal(&svc, req.request_id).await;
    assert_eq!(done.state, "cancelled");
    assert!(!done.mutation.unwrap().partial);
    assert_eq!(fs::read(&dst).await.unwrap(), b"original");
    assert!(!std::fs::read_dir(dir.path()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".pab-file-")
    }));
}

#[tokio::test]
async fn cancel_move_after_publication_preserves_both_copies() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let src = dir.path().join("source");
    let dst = dir.path().join("target");
    fs::write(&src, b"content").await.unwrap();
    let gate = gate(&svc, "before_source_delete").await;
    let req = request(&src, moving(&dst));
    execute(&svc, &req).await;
    gate.started.notified().await;
    svc.cancel_hash(actor(), req.request_id).await.unwrap();
    gate.resume.notify_one();
    let done = terminal(&svc, req.request_id).await;
    assert_eq!(done.state, "cancelled");
    assert!(done.mutation.unwrap().partial);
    assert_eq!(fs::read(&src).await.unwrap(), b"content");
    assert_eq!(fs::read(&dst).await.unwrap(), b"content");
}

#[tokio::test]
async fn delete_is_planned_recursive_explicit_and_new_children_survive() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let src = dir.path().join("source");
    fs::create_dir(&src).await.unwrap();
    fs::write(src.join("known"), b"known").await.unwrap();
    let action = |recursive| FileSystemAction::Delete {
        recursive,
        limits: FileOperationLimits::default(),
    };
    assert_eq!(
        run(&svc, &request(&src, action(false)))
            .await
            .error
            .unwrap()
            .code,
        "recursive_required"
    );
    let gate = gate(&svc, "before_delete").await;
    let req = request(&src, action(true));
    execute(&svc, &req).await;
    gate.started.notified().await;
    fs::write(src.join("new"), b"keep").await.unwrap();
    *svc.bulk_test_gate.lock().await = None;
    gate.resume.notify_one();
    let done = terminal(&svc, req.request_id).await;
    assert_eq!(done.state, "failed");
    let summary = done.mutation.unwrap();
    assert!(summary.partial);
    assert_eq!(summary.deleted_entries, 1);
    assert_eq!(fs::read(src.join("new")).await.unwrap(), b"keep");
    assert!(!src.join("known").exists());
    let done = run(&svc, &request(&src, action(true))).await;
    assert_eq!(done.state, "completed");
    assert!(!src.exists());
    let root = src.ancestors().last().unwrap();
    assert_eq!(
        run(&svc, &request(root, action(true)))
            .await
            .error
            .unwrap()
            .code,
        "root_not_supported"
    );
}

#[tokio::test]
async fn zip_roundtrip_unicode_empty_dirs_binary_and_overwrite_preflight() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let src = dir.path().join("中文");
    let archive = dir.path().join("out.zip");
    let dst = dir.path().join("extract");
    fs::create_dir_all(src.join("empty")).await.unwrap();
    fs::write(src.join("数据.bin"), [0, 255, 1, 2])
        .await
        .unwrap();
    let req = request(
        &archive,
        FileSystemAction::ArchiveCreate {
            sources: vec![src.to_str().unwrap().to_owned()],
            overwrite: false,
            limits: FileOperationLimits::default(),
        },
    );
    let done = run(&svc, &req).await;
    assert_eq!(done.state, "completed", "{done:?}");
    assert_eq!(
        done.metadata.unwrap().sha256,
        Some(digest(&fs::read(&archive).await.unwrap()))
    );
    let done = run(&svc, &request(&archive, extract(&dst))).await;
    assert_eq!(done.state, "completed", "{done:?}");
    assert!(dst.join("中文/empty").is_dir());
    assert_eq!(
        fs::read(dst.join("中文/数据.bin")).await.unwrap(),
        [0, 255, 1, 2]
    );
    let done = run(&svc, &request(&archive, extract(&dst))).await;
    assert_eq!(done.error.unwrap().code, "already_exists");
    assert_eq!(done.mutation.unwrap().published_entries, 0);
}

#[tokio::test]
async fn zip_unsafe_names_and_case_file_conflicts_fail_before_destination_creation() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let archive = dir.path().join("evil.zip");
    let dst = dir.path().join("extract");
    for names in [
        vec!["../outside"],
        vec!["/absolute"],
        vec!["C:/drive"],
        vec!["a\\b"],
        vec!["CON.txt"],
        vec!["invalid?.txt"],
        vec!["control\u{1}"],
        vec!["trailing."],
        vec!["a", "A"],
        vec!["a", "a/b"],
        vec!["A/one", "a/two"],
    ] {
        let entries: Vec<_> = names.iter().map(|name| (*name, b"x".as_slice())).collect();
        fs::write(&archive, zip_bytes(&entries, CompressionMethod::Stored))
            .await
            .unwrap();
        let done = run(&svc, &request(&archive, extract(&dst))).await;
        assert_eq!(done.state, "failed", "{names:?}: {done:?}");
        assert!(!dst.exists());
        assert_eq!(done.mutation.unwrap().published_entries, 0);
    }
    assert!(!dir.path().join("outside").exists());
}

#[tokio::test]
async fn zip_ratio_byte_depth_and_central_metadata_quotas_preflight() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let archive = dir.path().join("limit.zip");
    let dst = dir.path().join("extract");
    fs::write(
        &archive,
        zip_bytes(&[("bomb", &vec![0; 100_000])], CompressionMethod::Deflated),
    )
    .await
    .unwrap();
    assert_eq!(
        run(&svc, &request(&archive, extract(&dst)))
            .await
            .error
            .unwrap()
            .code,
        "compression_ratio_limit"
    );
    assert!(!dst.exists());
    let original = zip_bytes(&[("a/b/c", b"12345")], CompressionMethod::Stored);
    fs::write(&archive, &original).await.unwrap();
    for (bytes, depth, expected) in [(4, 64, "bytes_limit"), (100, 2, "depth_limit")] {
        let mut action = extract(&dst);
        if let FileSystemAction::ArchiveExtract { limits, .. } = &mut action {
            limits.max_bytes = bytes;
            limits.max_depth = depth;
        }
        assert_eq!(
            run(&svc, &request(&archive, action))
                .await
                .error
                .unwrap()
                .code,
            expected
        );
        assert!(!dst.exists());
    }
    for (field, data) in [
        (10, 4097u32.to_le_bytes().to_vec()[..2].to_vec()),
        (12, (2 * 1024 * 1024 + 1u32).to_le_bytes().to_vec()),
    ] {
        let mut bytes = original.clone();
        let eocd = bytes.len() - 22;
        bytes[eocd + field..eocd + field + data.len()].copy_from_slice(&data);
        if field == 10 {
            bytes[eocd + 8..eocd + 10].copy_from_slice(&data);
        }
        fs::write(&archive, bytes).await.unwrap();
        assert_eq!(
            run(&svc, &request(&archive, extract(&dst)))
                .await
                .error
                .unwrap()
                .code,
            "archive_metadata_limit"
        );
        assert!(!dst.exists());
    }
}

#[tokio::test]
async fn corrupt_later_zip_entry_reports_partial_and_never_publishes_bad_file() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let archive = dir.path().join("corrupt.zip");
    let dst = dir.path().join("extract");
    let mut bytes = zip_bytes(
        &[("a-good", b"GOOD-DATA"), ("z-bad", b"BAD-DATA")],
        CompressionMethod::Stored,
    );
    let i = bytes.windows(8).position(|b| b == b"BAD-DATA").unwrap();
    bytes[i] ^= 0xff;
    fs::write(&archive, bytes).await.unwrap();
    let done = run(&svc, &request(&archive, extract(&dst))).await;
    assert_eq!(done.state, "failed");
    assert!(done.mutation.unwrap().partial);
    assert_eq!(fs::read(dst.join("a-good")).await.unwrap(), b"GOOD-DATA");
    assert!(!dst.join("z-bad").exists());
}

#[tokio::test]
async fn bulk_quota_and_restart_are_durable_and_do_not_replay_mutations() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let src = dir.path().join("source");
    let dst = dir.path().join("target");
    fs::write(&src, b"content").await.unwrap();
    let permits = svc.bulk_slots.clone().acquire_many_owned(4).await.unwrap();
    let req = request(&src, copy(&dst, false, false));
    assert_eq!(
        execute(&svc, &req).await.error.unwrap().code,
        "executor_busy"
    );
    drop(permits);
    assert_eq!(execute(&svc, &req).await.state, "failed");
    assert!(!dst.exists());
    let req = request(&src, moving(&dst));
    svc.store
        .accept_filesystem(actor(), &req, &digest(&serde_json::to_vec(&req).unwrap()))
        .await
        .unwrap();
    svc.store.interrupt_read_operations().await.unwrap();
    assert_eq!(execute(&svc, &req).await.state, "unconfirmed");
    assert!(src.exists());
    assert!(!dst.exists());
}

#[tokio::test]
async fn bulk_quic_accept_query_cancel_have_independent_streams() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let src = dir.path().join("source");
    let dst = dir.path().join("target");
    fs::write(&src, b"content").await.unwrap();
    let gate = gate(&svc, "before_publish").await;
    let req = request(&src, copy(&dst, false, false));
    let (a, b, client, server) = pair().await;
    for (wire, expected) in [
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
            .send_json(&wire, Duration::from_secs(5))
            .await
            .unwrap();
        let response: DeviceTaskResponse =
            stream.receive_json(Duration::from_secs(5)).await.unwrap();
        assert!(matches!(response,DeviceTaskResponse::FileSystem {reply} if reply.state==expected));
        stream
            .expect_receive_end(Duration::from_secs(5))
            .await
            .unwrap();
        worker.await.unwrap();
        if expected == "running" && matches!(wire, DeviceTaskRequest::FileSystem { .. }) {
            gate.started.notified().await;
        }
    }
    gate.resume.notify_one();
    assert_eq!(terminal(&svc, req.request_id).await.state, "cancelled");
    assert!(!dst.exists());
    a.close().await;
    b.close().await;
}

#[tokio::test]
async fn zip_implicit_directories_and_archive_links_are_preflight_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let archive = dir.path().join("archive.zip");
    let dst = dir.path().join("extract");
    fs::write(
        &archive,
        zip_bytes(&[("a/b/c/d", b"x")], CompressionMethod::Stored),
    )
    .await
    .unwrap();
    let mut action = extract(&dst);
    if let FileSystemAction::ArchiveExtract { limits, .. } = &mut action {
        limits.max_entries = 3;
    }
    assert_eq!(
        run(&svc, &request(&archive, action))
            .await
            .error
            .unwrap()
            .code,
        "entry_limit"
    );
    assert!(!dst.exists());
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .add_symlink("link", "../outside", SimpleFileOptions::default())
        .unwrap();
    fs::write(&archive, writer.finish().unwrap().into_inner())
        .await
        .unwrap();
    assert_eq!(
        run(&svc, &request(&archive, extract(&dst)))
            .await
            .error
            .unwrap()
            .code,
        "unsupported_archive_entry"
    );
    assert!(!dst.exists());
    let original = zip_bytes(&[("normal", b"x")], CompressionMethod::Stored);
    let mut encrypted = original.clone();
    let central = encrypted
        .windows(4)
        .position(|b| b == b"PK\x01\x02")
        .unwrap();
    encrypted[6] |= 1;
    encrypted[central + 8] |= 1;
    fs::write(&archive, encrypted).await.unwrap();
    assert_eq!(
        run(&svc, &request(&archive, extract(&dst)))
            .await
            .error
            .unwrap()
            .code,
        "unsupported_archive_format"
    );
    assert!(!dst.exists());
    let mut zip64 = original;
    let eocd = zip64.len() - 22;
    zip64[eocd + 8..eocd + 12].fill(255);
    fs::write(&archive, zip64).await.unwrap();
    assert_eq!(
        run(&svc, &request(&archive, extract(&dst))).await.state,
        "failed"
    );
    assert!(!dst.exists());
}

#[tokio::test]
async fn replaced_empty_directory_is_not_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let path = dir.path().join("delete");
    fs::create_dir(&path).await.unwrap();
    let gate = gate(&svc, "before_delete").await;
    let req = request(
        &path,
        FileSystemAction::Delete {
            recursive: false,
            limits: FileOperationLimits::default(),
        },
    );
    execute(&svc, &req).await;
    gate.started.notified().await;
    fs::rename(&path, dir.path().join("original"))
        .await
        .unwrap();
    fs::create_dir(&path).await.unwrap();
    gate.resume.notify_one();
    let reply = terminal(&svc, req.request_id).await;
    assert_eq!(reply.error.unwrap().code, "version_conflict");
    assert!(path.is_dir());
    assert!(dir.path().join("original").is_dir());
}

#[tokio::test]
async fn bulk_item_output_truncates_without_losing_counts() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let path = dir.path().join("delete");
    fs::create_dir(&path).await.unwrap();
    for n in 0..70 {
        fs::write(path.join(format!("{n}")), []).await.unwrap();
    }
    let reply = run(
        &svc,
        &request(
            &path,
            FileSystemAction::Delete {
                recursive: true,
                limits: FileOperationLimits::default(),
            },
        ),
    )
    .await;
    assert_eq!(reply.state, "completed");
    let m = reply.mutation.as_ref().unwrap();
    assert_eq!(m.deleted_entries, 71);
    assert!(m.results_truncated);
    assert!(m.results.len() <= 64);
    assert!(serde_json::to_vec(&reply).unwrap().len() < 32 * 1024);
}
