use super::*;
use crate::task_service::transfer_tests::{actor, pair, service};
use pab_protocol::{
    DEVICE_TASK_SCHEMA_VERSION, DeviceTaskRequest, TextEdit, TextEncoding, TextReadRange,
};

fn request(path: &Path, operation: FileSystemAction, payload: &[u8]) -> FileSystemRequest {
    FileSystemRequest {
        execution: Default::default(),
        request_id: RequestId::new(),
        path: path.to_string_lossy().into_owned(),
        payload_size: payload.len() as u32,
        payload_sha256: operation.has_payload().then(|| digest(payload)),
        operation,
    }
}

async fn execute(
    service: &TaskService,
    request: &FileSystemRequest,
    payload: &[u8],
) -> FileSystemReply {
    service
        .execute_filesystem(actor(), request, payload)
        .await
        .unwrap()
        .0
}

#[tokio::test]
#[cfg(unix)]
async fn directory_escaped_names_return_bounded_error_and_smaller_pages_work() {
    let dir = tempfile::tempdir().unwrap();
    let service = service(dir.path()).await;
    let files = dir.path().join("escaped");
    tokio::fs::create_dir(&files).await.unwrap();
    for index in 0..64 {
        tokio::fs::write(
            files.join(format!("{index:02}{}", "\u{1}".repeat(200))),
            b"",
        )
        .await
        .unwrap();
    }
    let make = |limit| {
        request(
            &files,
            FileSystemAction::ListDirectory { after: None, limit },
            &[],
        )
    };
    let large = execute(&service, &make(64), &[]).await;
    assert_eq!(large.state, "failed");
    assert_eq!(
        large.error.as_ref().unwrap().code,
        "directory_page_too_large"
    );
    assert!(large.directory.is_none());
    assert!(serde_json::to_vec(&large).unwrap().len() < 32 * 1024);
    let small = execute(&service, &make(1), &[]).await;
    assert_eq!(small.state, "completed");
    let page = small.directory.unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(
        page.next_after.as_deref(),
        Some(page.entries[0].name.as_str())
    );
}

#[tokio::test]
async fn write_read_patch_preserves_bom_newlines_and_permissions_for_all_encodings() {
    let dir = tempfile::tempdir().unwrap();
    let service = service(dir.path()).await;
    for encoding in [
        TextEncoding::Utf8,
        TextEncoding::Utf8Bom,
        TextEncoding::Utf16Le,
        TextEncoding::Utf16Be,
    ] {
        let path = dir.path().join(format!("中文 file-{encoding:?}.txt"));
        let content = "甲😀\r\noriginal\r\n";
        let write = request(
            &path,
            FileSystemAction::Write {
                encoding,
                overwrite: false,
                expected_hash: None,
            },
            content.as_bytes(),
        );
        let reply = execute(&service, &write, content.as_bytes()).await;
        assert_eq!(reply.state, "completed");
        let hash = reply.metadata.unwrap().sha256.unwrap();
        let read = request(
            &path,
            FileSystemAction::Read {
                range: TextReadRange::Lines {
                    start_line: 1,
                    count: 1,
                },
                encoding: None,
                expected_hash: Some(hash.clone()),
            },
            &[],
        );
        let (reply, data) = service
            .execute_filesystem(actor(), &read, &[])
            .await
            .unwrap();
        assert_eq!(reply.state, "completed");
        assert_eq!(String::from_utf8(data).unwrap(), "甲😀\r\n");
        assert_eq!(reply.metadata.unwrap().newline.as_deref(), Some("crlf"));
        let permissions = fs::metadata(&path).await.unwrap().permissions();
        let edits = serde_json::to_vec(&vec![TextEdit {
            find: "original".to_owned(),
            replace: "修改".to_owned(),
            expected_matches: 1,
        }])
        .unwrap();
        let patch = request(
            &path,
            FileSystemAction::Patch {
                dry_run: false,
                expected_hash: hash,
                encoding: None,
            },
            &edits,
        );
        assert_eq!(execute(&service, &patch, &edits).await.state, "completed");
        let final_bytes = fs::read(&path).await.unwrap();
        assert_eq!(
            final_bytes,
            text::encode("甲😀\r\n修改\r\n", encoding, true).unwrap()
        );
        assert_eq!(
            fs::metadata(&path).await.unwrap().permissions(),
            permissions
        );
    }
}

#[tokio::test]
async fn conflicts_leave_the_original_bytes_and_no_staging_file() {
    let dir = tempfile::tempdir().unwrap();
    let service = service(dir.path()).await;
    let path = dir.path().join("shared.txt");
    fs::write(&path, b"old old\r\n").await.unwrap();
    let original = fs::read(&path).await.unwrap();
    let hash = digest(&original);
    for (operation, payload, code) in [
        (
            FileSystemAction::Write {
                encoding: TextEncoding::Utf8,
                overwrite: false,
                expected_hash: None,
            },
            b"new".to_vec(),
            "already_exists",
        ),
        (
            FileSystemAction::Write {
                encoding: TextEncoding::Utf8,
                overwrite: true,
                expected_hash: Some("a".repeat(64)),
            },
            b"new".to_vec(),
            "version_conflict",
        ),
        (
            FileSystemAction::Patch {
                dry_run: false,
                expected_hash: hash.clone(),
                encoding: None,
            },
            serde_json::to_vec(&vec![TextEdit {
                find: "old".to_owned(),
                replace: "new".to_owned(),
                expected_matches: 1,
            }])
            .unwrap(),
            "match_conflict",
        ),
        (
            FileSystemAction::Patch {
                dry_run: false,
                expected_hash: hash.clone(),
                encoding: None,
            },
            serde_json::to_vec(&vec![TextEdit {
                find: "absent".to_owned(),
                replace: "new".to_owned(),
                expected_matches: 1,
            }])
            .unwrap(),
            "match_conflict",
        ),
    ] {
        let reply = execute(&service, &request(&path, operation, &payload), &payload).await;
        assert_eq!(reply.error.unwrap().code, code);
        assert_eq!(fs::read(&path).await.unwrap(), original);
    }
    let mut reader = fs::read_dir(dir.path()).await.unwrap();
    while let Some(entry) = reader.next_entry().await.unwrap() {
        assert!(
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".pab-text-")
        );
    }
}

#[tokio::test]
async fn request_dedup_survives_reopen_and_never_replays_after_external_edit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dedup.txt");
    let first = service(dir.path()).await;
    let request = request(
        &path,
        FileSystemAction::Write {
            encoding: TextEncoding::Utf8,
            overwrite: true,
            expected_hash: None,
        },
        b"accepted",
    );
    assert_eq!(
        execute(&first, &request, b"accepted").await.state,
        "completed"
    );
    drop(first);
    fs::write(&path, b"external change").await.unwrap();
    let reopened = service(dir.path()).await;
    assert_eq!(
        execute(&reopened, &request, b"accepted").await.state,
        "completed"
    );
    assert_eq!(fs::read(&path).await.unwrap(), b"external change");
    let mut changed = request.clone();
    changed.payload_sha256 = Some(digest(b"different"));
    assert!(matches!(
        reopened
            .execute_filesystem(actor(), &changed, b"different")
            .await,
        Err(TaskServiceError::Store(
            crate::task_store::TaskStoreError::RequestConflict
        ))
    ));
    let other = pab_protocol::OperatorRef::account(
        pab_protocol::UserId::from_u128(99),
        pab_protocol::EndpointKey::new([99; 32]),
    );
    assert!(
        reopened
            .lookup_filesystem(other, request.request_id)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn changed_read_version_binary_and_malformed_utf16_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let service = service(dir.path()).await;
    let path = dir.path().join("read.txt");
    fs::write(&path, b"first").await.unwrap();
    let version = digest(b"first");
    fs::write(&path, b"changed").await.unwrap();
    let make_read = |hash| {
        request(
            &path,
            FileSystemAction::Read {
                range: TextReadRange::Bytes {
                    offset: 0,
                    max_bytes: 64,
                },
                encoding: None,
                expected_hash: hash,
            },
            &[],
        )
    };
    assert_eq!(
        execute(&service, &make_read(Some(version)), &[])
            .await
            .error
            .unwrap()
            .code,
        "version_conflict"
    );
    for bytes in [
        vec![0, 1, 255],
        vec![0xff, 0xfe, 0],
        vec![0xff, 0xfe, 0, 0xd8],
    ] {
        fs::write(&path, bytes).await.unwrap();
        assert_eq!(
            execute(&service, &make_read(None), &[]).await.state,
            "failed"
        );
    }
}

#[test]
fn ranges_preserve_codepoints_and_raw_offsets_across_bom_and_surrogates() {
    for encoding in [
        TextEncoding::Utf8,
        TextEncoding::Utf8Bom,
        TextEncoding::Utf16Le,
        TextEncoding::Utf16Be,
    ] {
        let bytes = text::encode("中😀A\r\nB\rC\nD", encoding, true).unwrap();
        let doc = text::decode(&bytes, None).unwrap();
        let mut offset = 0;
        let mut output = Vec::new();
        loop {
            let (data, position) = text::read(
                &doc,
                &TextReadRange::Bytes {
                    offset,
                    max_bytes: 4,
                },
            )
            .unwrap();
            assert!(std::str::from_utf8(&data).is_ok());
            output.extend(data);
            offset = position.next_offset;
            if !position.truncated {
                break;
            }
        }
        assert_eq!(String::from_utf8(output).unwrap(), doc.text);
        assert_eq!(offset, bytes.len() as u64);
        let (data, _) = text::read(
            &doc,
            &TextReadRange::Lines {
                start_line: 2,
                count: 2,
            },
        )
        .unwrap();
        assert_eq!(data, b"B\rC\n");
        assert!(
            text::read(
                &doc,
                &TextReadRange::Bytes {
                    offset: doc.bom as u64 + 1,
                    max_bytes: 4
                }
            )
            .is_err()
        );
    }
}

#[test]
fn overlong_line_can_continue_by_versioned_byte_offset_and_empty_files_work() {
    let bytes = "中".repeat(10000).into_bytes();
    let doc = text::decode(&bytes, None).unwrap();
    let (first, position) = text::read(
        &doc,
        &TextReadRange::Lines {
            start_line: 1,
            count: 1,
        },
    )
    .unwrap();
    assert!(first.len() <= pab_protocol::MAX_TEXT_READ_BYTES);
    assert!(position.truncated);
    assert!(position.next_line.is_none());
    let (next, _) = text::read(
        &doc,
        &TextReadRange::Bytes {
            offset: position.next_offset,
            max_bytes: 16384,
        },
    )
    .unwrap();
    assert_eq!([first, next].concat(), bytes);
    let doc = text::decode(b"", None).unwrap();
    assert!(
        text::read(
            &doc,
            &TextReadRange::Lines {
                start_line: 1,
                count: 1
            }
        )
        .unwrap()
        .0
        .is_empty()
    );
}

#[test]
fn patches_use_original_spans_and_reject_overlap_and_implicit_encoding_conversion() {
    let edit = |find: &str, replace: &str| TextEdit {
        find: find.to_owned(),
        replace: replace.to_owned(),
        expected_matches: 1,
    };
    assert!(text::patch("abc", &[edit("ab", "x"), edit("bc", "y")]).is_err());
    assert!(text::patch("a", &[edit("a", "b"), edit("b", "c")]).is_err());
    assert_eq!(
        text::patch("ab", &[edit("a", "b"), edit("b", "c")]).unwrap(),
        "bc"
    );
    assert!(text::decode(&[0xff, 0xfe, 65, 0], Some(TextEncoding::Utf8)).is_err());
}

#[tokio::test]
async fn shared_upload_lock_blocks_text_mutation_and_other_paths_remain_available() {
    let dir = tempfile::tempdir().unwrap();
    let service = service(dir.path()).await;
    let path = dir.path().join("shared.txt");
    fs::write(&path, b"original").await.unwrap();
    let held = service
        .upload_locks
        .try_acquire(&path)
        .await
        .unwrap()
        .unwrap();
    let write = request(
        &path,
        FileSystemAction::Write {
            encoding: TextEncoding::Utf8,
            overwrite: true,
            expected_hash: None,
        },
        b"new",
    );
    assert_eq!(
        execute(&service, &write, b"new").await.error.unwrap().code,
        "path_busy"
    );
    let other = request(
        &dir.path().join("other.txt"),
        FileSystemAction::Write {
            encoding: TextEncoding::Utf8,
            overwrite: false,
            expected_hash: None,
        },
        b"new",
    );
    assert_eq!(execute(&service, &other, b"new").await.state, "completed");
    assert_eq!(fs::read(&path).await.unwrap(), b"original");
    drop(held);
}

#[tokio::test]
async fn publication_intent_recovers_after_restart_without_reexecution() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("recover.txt");
    let first = service(dir.path()).await;
    let request = request(
        &path,
        FileSystemAction::Write {
            encoding: TextEncoding::Utf8,
            overwrite: false,
            expected_hash: None,
        },
        b"published",
    );
    let fingerprint = digest(&serde_json::to_vec(&request).unwrap());
    first
        .store
        .accept_filesystem(actor(), &request, &fingerprint)
        .await
        .unwrap();
    let mut intent = FileSystemReply::pending(&request);
    let bytes = b"published";
    fs::write(&path, bytes).await.unwrap();
    let mut meta = metadata(&fs::metadata(&path).await.unwrap());
    meta.sha256 = Some(digest(bytes));
    intent.metadata = Some(meta);
    first.store.begin_file_publication(&intent).await.unwrap();
    drop(first);
    let reopened = service(dir.path()).await;
    assert_eq!(
        reopened
            .lookup_filesystem(actor(), request.request_id)
            .await
            .unwrap()
            .state,
        "completed"
    );
    assert_eq!(fs::read(&path).await.unwrap(), bytes);
}

#[tokio::test]
async fn real_quic_carries_large_text_as_binary_and_records_only_summaries() {
    let dir = tempfile::tempdir().unwrap();
    let service = service(dir.path()).await;
    let path = dir.path().join("wire.txt");
    let payload = "秘".repeat(30000).into_bytes();
    let request = request(
        &path,
        FileSystemAction::Write {
            encoding: TextEncoding::Utf8,
            overwrite: false,
            expected_hash: None,
        },
        &payload,
    );
    let (client_endpoint, server_endpoint, client, server) = pair().await;
    let worker_service = service.clone();
    let worker_connection = server.clone();
    let worker = tokio::spawn(async move {
        worker_service
            .handle_stream(
                actor(),
                worker_connection
                    .accept_bi(Duration::from_secs(5))
                    .await
                    .unwrap(),
                Duration::from_secs(5),
            )
            .await
            .unwrap();
    });
    let mut stream = client.open_bi(Duration::from_secs(5)).await.unwrap();
    stream
        .send_frame_json(
            &DeviceTaskRequest::FileSystem {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request: request.clone(),
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    for chunk in payload.chunks(65536) {
        stream
            .send_binary_frame(chunk, Duration::from_secs(5))
            .await
            .unwrap();
    }
    stream.finish_send(Duration::from_secs(5)).await.unwrap();
    let reply: DeviceTaskResponse = stream.receive_json(Duration::from_secs(5)).await.unwrap();
    assert!(
        matches!(reply, DeviceTaskResponse::FileSystem { reply } if reply.state == "completed")
    );
    worker.await.unwrap();
    assert_eq!(fs::read(&path).await.unwrap(), payload);
    let summary = serde_json::to_string(
        &service
            .lookup_filesystem(actor(), request.request_id)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(!summary.contains("秘"));
    let read_request = self::request(
        &path,
        FileSystemAction::Read {
            range: TextReadRange::Bytes {
                offset: 0,
                max_bytes: 16384,
            },
            encoding: None,
            expected_hash: Some(digest(&payload)),
        },
        &[],
    );
    let worker_service = service.clone();
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
        .send_json(
            &DeviceTaskRequest::FileSystem {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request: read_request,
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    let reply: DeviceTaskResponse = stream.receive_json(Duration::from_secs(5)).await.unwrap();
    let DeviceTaskResponse::FileSystem { reply } = reply else {
        panic!("wrong filesystem response")
    };
    let data = stream
        .receive_binary_frame(Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(reply.data_size as usize, data.len());
    assert_eq!(reply.data_sha256, Some(digest(&data)));
    assert!(data.len() <= 16384);
    assert!(std::str::from_utf8(&data).is_ok());
    assert!(reply.range.unwrap().truncated);
    stream
        .expect_receive_end(Duration::from_secs(5))
        .await
        .unwrap();
    worker.await.unwrap();
    client_endpoint.close().await;
    server_endpoint.close().await;
}

#[tokio::test]
async fn bomless_utf16_patch_keeps_the_explicit_encoding_and_missing_bom() {
    let dir = tempfile::tempdir().unwrap();
    let service = service(dir.path()).await;
    let path = dir.path().join("bomless.txt");
    let original = text::encode("original\r\n", TextEncoding::Utf16Le, false).unwrap();
    fs::write(&path, &original).await.unwrap();
    let edits = serde_json::to_vec(&vec![TextEdit {
        find: "original".to_owned(),
        replace: "修改".to_owned(),
        expected_matches: 1,
    }])
    .unwrap();
    let req = request(
        &path,
        FileSystemAction::Patch {
            dry_run: false,
            expected_hash: digest(&original),
            encoding: Some(TextEncoding::Utf16Le),
        },
        &edits,
    );
    assert_eq!(execute(&service, &req, &edits).await.state, "completed");
    assert_eq!(
        fs::read(&path).await.unwrap(),
        text::encode("修改\r\n", TextEncoding::Utf16Le, false).unwrap()
    );
}

#[tokio::test]
async fn real_quic_rejects_trailing_payload_before_acceptance_or_publication() {
    let dir = tempfile::tempdir().unwrap();
    let service = service(dir.path()).await;
    let path = dir.path().join("extra.txt");
    fs::write(&path, b"original").await.unwrap();
    let req = request(
        &path,
        FileSystemAction::Write {
            encoding: TextEncoding::Utf8,
            overwrite: true,
            expected_hash: None,
        },
        b"new",
    );
    let (a, b, client, server) = pair().await;
    let worker_service = service.clone();
    let worker = tokio::spawn(async move {
        worker_service
            .handle_stream(
                actor(),
                server.accept_bi(Duration::from_secs(5)).await.unwrap(),
                Duration::from_secs(5),
            )
            .await
    });
    let mut stream = client.open_bi(Duration::from_secs(5)).await.unwrap();
    stream
        .send_frame_json(
            &DeviceTaskRequest::FileSystem {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request: req.clone(),
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    stream
        .send_binary_frame(b"new", Duration::from_secs(5))
        .await
        .unwrap();
    stream
        .send_binary_frame(b"extra", Duration::from_secs(5))
        .await
        .unwrap();
    let _ = stream.finish_send(Duration::from_secs(5)).await;
    assert!(worker.await.unwrap().is_err());
    assert_eq!(fs::read(&path).await.unwrap(), b"original");
    assert!(
        service
            .lookup_filesystem(actor(), req.request_id)
            .await
            .is_err()
    );
    a.close().await;
    b.close().await;
}

#[tokio::test]
async fn recovery_without_publication_evidence_stays_unconfirmed_and_does_not_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unknown.txt");
    let first = service(dir.path()).await;
    let req = request(
        &path,
        FileSystemAction::Write {
            encoding: TextEncoding::Utf8,
            overwrite: false,
            expected_hash: None,
        },
        b"planned",
    );
    let fingerprint = digest(&serde_json::to_vec(&req).unwrap());
    first
        .store
        .accept_filesystem(actor(), &req, &fingerprint)
        .await
        .unwrap();
    drop(first);
    let reopened = service(dir.path()).await;
    assert_eq!(
        execute(&reopened, &req, b"planned").await.state,
        "unconfirmed"
    );
    assert!(!path.exists());
}

#[tokio::test]
async fn overload_rejects_before_acceptance_and_old_environment_payload_still_decodes() {
    let dir = tempfile::tempdir().unwrap();
    let service = service(dir.path()).await;
    let response = service
        .handle_request(
            actor(),
            DeviceTaskRequest::GetEnvironment {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        &response,
        DeviceTaskResponse::Environment {
            filesystem_schema_version: Some(6),
            ..
        }
    ));
    let mut old = serde_json::to_value(response).unwrap();
    old.as_object_mut()
        .unwrap()
        .remove("filesystem_schema_version");
    assert!(matches!(
        serde_json::from_value::<DeviceTaskResponse>(old).unwrap(),
        DeviceTaskResponse::Environment {
            filesystem_schema_version: None,
            ..
        }
    ));
    let req = request(
        &dir.path().join("busy.txt"),
        FileSystemAction::Stat {
            follow_symlinks: false,
        },
        &[],
    );
    let _held = service.filesystem_slots.acquire_many(8).await.unwrap();
    let (a, b, client, server) = pair().await;
    let worker_service = service.clone();
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
        .send_json(
            &DeviceTaskRequest::FileSystem {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request: req.clone(),
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    let response: DeviceTaskResponse = stream.receive_json(Duration::from_secs(5)).await.unwrap();
    assert!(
        matches!(response, DeviceTaskResponse::FileSystem { reply } if reply.error.as_ref().unwrap().code == "executor_busy")
    );
    worker.await.unwrap();
    assert!(
        service
            .lookup_filesystem(actor(), req.request_id)
            .await
            .is_err()
    );
    a.close().await;
    b.close().await;
}

#[tokio::test]
async fn exact_file_limit_utf16_expansion_and_empty_writes_are_supported() {
    let dir = tempfile::tempdir().unwrap();
    let service = service(dir.path()).await;
    for encoding in [
        TextEncoding::Utf8,
        TextEncoding::Utf8Bom,
        TextEncoding::Utf16Le,
        TextEncoding::Utf16Be,
    ] {
        let path = dir.path().join(format!("empty-{encoding:?}.txt"));
        let req = request(
            &path,
            FileSystemAction::Write {
                encoding,
                overwrite: false,
                expected_hash: None,
            },
            b"",
        );
        assert_eq!(execute(&service, &req, b"").await.state, "completed");
        assert_eq!(
            fs::read(&path).await.unwrap(),
            text::encode("", encoding, true).unwrap()
        );
    }
    let path = dir.path().join("max-utf16.txt");
    let text = "中".repeat(pab_protocol::MAX_TEXT_FILE_BYTES / 2 - 2) + "末";
    let original = super::text::encode(&text, TextEncoding::Utf16Le, true).unwrap();
    assert_eq!(original.len(), pab_protocol::MAX_TEXT_FILE_BYTES);
    fs::write(&path, &original).await.unwrap();
    let edits = serde_json::to_vec(&vec![TextEdit {
        find: "末".to_owned(),
        replace: "改".to_owned(),
        expected_matches: 1,
    }])
    .unwrap();
    let req = request(
        &path,
        FileSystemAction::Patch {
            dry_run: false,
            expected_hash: digest(&original),
            encoding: None,
        },
        &edits,
    );
    assert_eq!(execute(&service, &req, &edits).await.state, "completed");
    assert_eq!(
        fs::metadata(&path).await.unwrap().len(),
        pab_protocol::MAX_TEXT_FILE_BYTES as u64
    );
}

#[tokio::test]
async fn directory_readonly_missing_parent_and_size_limits_do_not_damage_files() {
    let dir = tempfile::tempdir().unwrap();
    let service = service(dir.path()).await;
    for path in [
        dir.path().to_path_buf(),
        dir.path().join("absent").join("file.txt"),
    ] {
        let write = request(
            &path,
            FileSystemAction::Write {
                encoding: TextEncoding::Utf8,
                overwrite: true,
                expected_hash: None,
            },
            b"new",
        );
        assert_eq!(execute(&service, &write, b"new").await.state, "failed");
    }
    let path = dir.path().join("readonly.txt");
    fs::write(&path, b"protected").await.unwrap();
    let mut permissions = fs::metadata(&path).await.unwrap().permissions();
    let original_permissions = permissions.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).await.unwrap();
    let write = request(
        &path,
        FileSystemAction::Write {
            encoding: TextEncoding::Utf8,
            overwrite: true,
            expected_hash: None,
        },
        b"new",
    );
    assert_eq!(
        execute(&service, &write, b"new").await.error.unwrap().code,
        "access_denied"
    );
    assert_eq!(fs::read(&path).await.unwrap(), b"protected");
    fs::set_permissions(&path, original_permissions)
        .await
        .unwrap();
    let file = fs::File::create(&path).await.unwrap();
    file.set_len(pab_protocol::MAX_TEXT_FILE_BYTES as u64 + 1)
        .await
        .unwrap();
    drop(file);
    let read = request(
        &path,
        FileSystemAction::Read {
            range: TextReadRange::Lines {
                start_line: 1,
                count: 1,
            },
            encoding: None,
            expected_hash: None,
        },
        &[],
    );
    assert_eq!(
        execute(&service, &read, &[]).await.error.unwrap().code,
        "file_too_large"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn link_and_parent_aliases_are_not_followed_for_text_mutations() {
    let dir = tempfile::tempdir().unwrap();
    let service = service(dir.path()).await;
    let path = dir.path().join("target.txt");
    fs::write(&path, b"protected").await.unwrap();
    let link = dir.path().join("alias.txt");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    let write = request(
        &link,
        FileSystemAction::Write {
            encoding: TextEncoding::Utf8,
            overwrite: true,
            expected_hash: None,
        },
        b"new",
    );
    assert_eq!(
        execute(&service, &write, b"new").await.error.unwrap().code,
        "link_not_supported"
    );
    assert_eq!(fs::read(&path).await.unwrap(), b"protected");
    let stat = request(
        &link,
        FileSystemAction::Stat {
            follow_symlinks: false,
        },
        &[],
    );
    let meta = execute(&service, &stat, &[]).await.metadata.unwrap();
    assert!(meta.is_link);
    assert_eq!(meta.kind, "symlink");
}
