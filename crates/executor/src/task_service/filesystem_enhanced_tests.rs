use super::*;
use crate::task_service::transfer_tests::{actor, pair, service};
use pab_protocol::{
    DEVICE_TASK_SCHEMA_VERSION, DeviceTaskRequest, FileSearchMode, FileSearchOptions, TextEncoding,
    TextReadRange,
};
use tokio::io::AsyncWriteExt;
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
fn follow(
    cursor: Option<String>,
    offset: Option<u64>,
    tail_bytes: Option<u32>,
    max_bytes: u32,
    wait_ms: u32,
) -> FileSystemAction {
    FileSystemAction::Read {
        range: TextReadRange::Stream {
            cursor,
            offset,
            tail_bytes,
            max_bytes,
            wait_ms,
        },
        encoding: None,
        expected_hash: None,
    }
}
async fn read(s: &TaskService, path: &Path, op: FileSystemAction) -> (FileSystemReply, Vec<u8>) {
    let req = request(path, op, &[]);
    req.validate().unwrap();
    s.execute_filesystem(actor(), &req, &[]).await.unwrap()
}
#[tokio::test]
async fn streaming_all_encodings_preserves_boundaries_and_eof_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    for enc in [
        TextEncoding::Utf8,
        TextEncoding::Utf8Bom,
        TextEncoding::Utf16Le,
        TextEncoding::Utf16Be,
    ] {
        let path = dir.path().join(format!("{enc:?}.log"));
        let source = "甲😀\r\n乙\n";
        fs::write(&path, text::encode(source, enc, true).unwrap())
            .await
            .unwrap();
        let mut cursor = None;
        let mut actual = Vec::new();
        for _ in 0..20 {
            let (r, data) = read(&svc, &path, follow(cursor, None, None, 4, 0)).await;
            assert_eq!(r.state, "completed", "{r:?}");
            actual.extend(data);
            cursor = Some(r.log.unwrap().cursor);
            if !r.range.unwrap().truncated {
                break;
            }
        }
        assert_eq!(String::from_utf8(actual).unwrap(), source, "{enc:?}");
        let (eof, data) = read(&svc, &path, follow(cursor, None, None, 4, 10)).await;
        assert!(data.is_empty());
        assert!(eof.log.unwrap().wait_expired);
    }
}
#[tokio::test]
async fn large_tail_wait_append_partial_character_and_rotation_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.log");
    let svc = service(dir.path()).await;
    let mut bytes = vec![b'x'; 5 * 1024 * 1024];
    bytes.extend_from_slice("\n甲😀\n".as_bytes());
    fs::write(&path, &bytes).await.unwrap();
    let (r, data) = read(&svc, &path, follow(None, None, Some(8), 16384, 0)).await;
    assert_eq!(r.state, "completed", "{r:?}");
    assert_eq!(String::from_utf8(data).unwrap(), "甲😀\n");
    assert!(r.metadata.unwrap().sha256.is_none());
    let cursor = r.log.unwrap().cursor;
    let request = request(
        &path,
        follow(Some(cursor.clone()), None, None, 16384, 1000),
        &[],
    );
    let service = svc.clone();
    let wait = tokio::spawn(async move {
        service
            .execute_filesystem(actor(), &request, &[])
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(60)).await;
    let mut writer = fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .await
        .unwrap();
    writer.write_all(&[0xe4, 0xb8]).await.unwrap();
    writer.flush().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!wait.is_finished());
    writer.write_all(&[0xad]).await.unwrap();
    writer.flush().await.unwrap();
    drop(writer);
    let (r, data) = wait.await.unwrap();
    assert_eq!(r.state, "completed", "{r:?}");
    assert_eq!(String::from_utf8(data).unwrap(), "中");
    let current = r.log.unwrap().cursor;
    fs::rename(&path, dir.path().join("old.log")).await.unwrap();
    fs::write(&path, b"replacement").await.unwrap();
    let (r, _) = read(&svc, &path, follow(Some(current), None, None, 16384, 0)).await;
    assert_eq!(r.error.unwrap().code, "log_changed");
    let (r, _) = read(&svc, &path, follow(None, None, None, 16384, 0)).await;
    let cursor = r.log.unwrap().cursor;
    fs::write(&path, b"r").await.unwrap();
    let (r, _) = read(&svc, &path, follow(Some(cursor), None, None, 16384, 0)).await;
    assert_eq!(r.error.unwrap().code, "log_changed");
}
#[tokio::test]
async fn log_cursor_rejects_wrong_path_corruption_and_interior_invalid_encoding() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let path = dir.path().join("a.log");
    fs::write(&path, b"abc").await.unwrap();
    let (r, _) = read(&svc, &path, follow(None, None, None, 8, 0)).await;
    let cursor = r.log.unwrap().cursor;
    let other = dir.path().join("b.log");
    fs::write(&other, b"abc").await.unwrap();
    let (r, _) = read(&svc, &other, follow(Some(cursor), None, None, 8, 0)).await;
    assert_eq!(r.error.unwrap().code, "log_changed");
    let (r, _) = read(
        &svc,
        &path,
        follow(Some("invalid".into()), None, None, 8, 0),
    )
    .await;
    assert_eq!(r.error.unwrap().code, "invalid_cursor");
    fs::write(&path, [b'a', 0xff, b'b']).await.unwrap();
    let (r, data) = read(&svc, &path, follow(None, None, None, 8, 0)).await;
    assert_eq!(r.error.unwrap().code, "invalid_encoding");
    assert!(data.is_empty());
}
fn search(cursor: Option<String>) -> FileSystemAction {
    FileSystemAction::Search {
        mode: FileSearchMode::Content,
        query: r"error\s+\d+".into(),
        glob: "**/*.txt".into(),
        case_sensitive: false,
        max_results: 1,
        max_depth: 8,
        max_file_bytes: 4194304,
        options: FileSearchOptions {
            regex: true,
            exclude: vec!["ignored/**".into()],
            context_lines: 1,
            cursor,
        },
    }
}
#[tokio::test]
async fn search_regex_exclusion_context_pages_and_tree_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    fs::create_dir_all(root.join("ignored")).await.unwrap();
    fs::write(
        root.join("a.txt"),
        "before\r\nERROR 1\r\nafter\r\nerror 2\n",
    )
    .await
    .unwrap();
    fs::write(root.join("b.txt"), "error 3\n").await.unwrap();
    fs::write(root.join("ignored/no.txt"), "error 4\n")
        .await
        .unwrap();
    let svc = service(dir.path()).await;
    let mut cursor = None;
    let mut found = Vec::new();
    let mut first = None;
    for _ in 0..8 {
        let (r, _) = read(&svc, &root, search(cursor)).await;
        assert_eq!(r.state, "completed", "{r:?}");
        let s = r.search.unwrap();
        if first.is_none() {
            first = s.next_cursor.clone();
            assert_eq!(s.matches[0].context[0].text, "before");
            assert_eq!(s.matches[0].context[2].text, "after");
        }
        for m in s.matches {
            found.push((
                Path::new(&m.path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                m.line.unwrap(),
            ));
        }
        cursor = s.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        found,
        vec![
            ("a.txt".into(), 2),
            ("a.txt".into(), 4),
            ("b.txt".into(), 1)
        ]
    );
    fs::write(root.join("new.txt"), "error 5").await.unwrap();
    let (r, _) = read(&svc, &root, search(first)).await;
    assert_eq!(r.error.unwrap().code, "search_changed");
    let mut invalid = search(None);
    if let FileSystemAction::Search { query, .. } = &mut invalid {
        *query = "[".into();
    }
    let (r, _) = read(&svc, &root, invalid).await;
    assert_eq!(r.error.unwrap().code, "invalid_regex");
}
#[tokio::test]
async fn patch_preview_dedup_conflicts_and_multi_edit_atomicity() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let path = dir.path().join("edit.txt");
    let original = text::encode("甲\r\n乙\r\n", TextEncoding::Utf16Le, true).unwrap();
    fs::write(&path, &original).await.unwrap();
    let edits = serde_json::to_vec(&vec![
        TextEdit {
            find: "甲".into(),
            replace: "一".into(),
            expected_matches: 1,
        },
        TextEdit {
            find: "乙".into(),
            replace: "二".into(),
            expected_matches: 1,
        },
    ])
    .unwrap();
    let preview = request(
        &path,
        FileSystemAction::Patch {
            expected_hash: digest(&original),
            encoding: None,
            dry_run: true,
        },
        &edits,
    );
    let (r, _) = svc
        .execute_filesystem(actor(), &preview, &edits)
        .await
        .unwrap();
    assert_eq!(r.state, "completed", "{r:?}");
    let evidence = r.patch_preview.unwrap();
    assert!(evidence.changed);
    assert_eq!(evidence.matched_edits, vec![1, 1]);
    assert_eq!(fs::read(&path).await.unwrap(), original);
    assert!(
        svc.execute_filesystem(actor(), &preview, &edits)
            .await
            .unwrap()
            .0
            .patch_preview
            .is_some()
    );
    let mut apply = preview.clone();
    apply.request_id = RequestId::new();
    if let FileSystemAction::Patch { dry_run, .. } = &mut apply.operation {
        *dry_run = false;
    }
    let (r, _) = svc
        .execute_filesystem(actor(), &apply, &edits)
        .await
        .unwrap();
    assert_eq!(r.state, "completed", "{r:?}");
    let updated = fs::read(&path).await.unwrap();
    assert_eq!(digest(&updated), evidence.result_sha256);
    assert_eq!(text::decode(&updated, None).unwrap().text, "一\r\n二\r\n");
    // The original request remains idempotent even when the path has changed later.
    fs::write(&path, b"external").await.unwrap();
    let (r, _) = svc
        .execute_filesystem(actor(), &apply, &edits)
        .await
        .unwrap();
    assert_eq!(r.state, "completed");
    assert_eq!(fs::read(&path).await.unwrap(), b"external");
    apply.request_id = RequestId::new();
    let (r, _) = svc
        .execute_filesystem(actor(), &apply, &edits)
        .await
        .unwrap();
    assert_eq!(r.error.unwrap().code, "version_conflict");
    assert_eq!(fs::read(&path).await.unwrap(), b"external");
    // A successful first edit must not publish if the second edit fails its match count.
    fs::write(&path, &original).await.unwrap();
    let bad = serde_json::to_vec(&vec![
        TextEdit {
            find: "甲".into(),
            replace: "一".into(),
            expected_matches: 1,
        },
        TextEdit {
            find: "missing".into(),
            replace: "二".into(),
            expected_matches: 1,
        },
    ])
    .unwrap();
    let req = request(&path, apply.operation, &bad);
    let (r, _) = svc.execute_filesystem(actor(), &req, &bad).await.unwrap();
    assert_eq!(r.error.unwrap().code, "match_conflict");
    assert_eq!(fs::read(&path).await.unwrap(), original);
}
#[tokio::test]
async fn enhanced_log_read_crosses_real_quic_binary_stream() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let path = dir.path().join("wire.log");
    fs::write(&path, "中文😀\n").await.unwrap();
    let req = request(&path, follow(None, None, Some(16384), 16384, 0), &[]);
    let (a, b, client, server) = pair().await;
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
        .send_frame_json(
            &DeviceTaskRequest::FileSystem {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request: req,
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    stream.finish_send(Duration::from_secs(5)).await.unwrap();
    let response: DeviceTaskResponse = stream.receive_json(Duration::from_secs(5)).await.unwrap();
    let DeviceTaskResponse::FileSystem { reply } = response else {
        panic!("unexpected response")
    };
    assert_eq!(reply.state, "completed", "{reply:?}");
    assert!(reply.log.is_some());
    let data = stream
        .receive_binary_frame(Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(data, "中文😀\n".as_bytes());
    stream
        .expect_receive_end(Duration::from_secs(5))
        .await
        .unwrap();
    worker.await.unwrap();
    a.close().await;
    b.close().await;
}
