use std::time::Duration;

use pab_platform::detect_native_execution_context;
use pab_protocol::{DeviceId, ExpectedEnvironment, RequestId, TaskState, TenantId, UserId};

use super::*;

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "run alone: verifies dispatch with no in-process desktop helper registered"]
async fn macos_key_release_reaches_helper_but_secure_attention_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let service = TaskService::open(
        &directory.path().join("tasks.sqlite3"),
        DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        },
        detect_native_execution_context().unwrap(),
    )
    .await
    .unwrap();
    let release = service
        .handle_request(
            account(9),
            DeviceTaskRequest::DesktopInput {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id: RequestId::new(),
                event: DesktopInputEvent::Key {
                    virtual_key: 91,
                    down: false,
                },
            },
        )
        .await;
    // The installed GUI is never contacted: helper registration is process-local.
    assert!(matches!(
        release,
        Err(TaskServiceError::WindowHelper(
            crate::local_ipc::LocalIpcError::WindowHelperUnavailable
        ))
    ));
    let secure = service
        .handle_request(
            account(9),
            DeviceTaskRequest::DesktopInput {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id: RequestId::new(),
                event: DesktopInputEvent::SecureAttention,
            },
        )
        .await;
    assert!(matches!(secure, Err(TaskServiceError::Unsupported)));
}

#[test]
fn desktop_permission_errors_are_actionable_without_exposing_helper_details() {
    for permission in [
        "screen_recording_permission_required",
        "accessibility_permission_required",
    ] {
        let response = error_response(&TaskServiceError::WindowHelper(
            crate::local_ipc::LocalIpcError::Remote(format!(
                "{permission}: private diagnostic details"
            )),
        ));
        let DeviceTaskResponse::Error { code, message } = response else {
            panic!("expected error");
        };
        assert_eq!(code, DeviceTaskErrorCode::AccessDenied);
        assert!(message.starts_with(permission));
        assert!(!message.contains("private diagnostic"));
    }
    let response = error_response(&TaskServiceError::WindowHelper(
        crate::local_ipc::LocalIpcError::Remote("/private/secret".into()),
    ));
    let DeviceTaskResponse::Error { message, .. } = response else {
        panic!("expected error");
    };
    assert!(!message.contains("/private/secret"));
}

#[test]
fn desktop_backend_failures_preserve_known_reasons_without_leaking_details() {
    for (backend, public) in [
        (
            "no connection could be established: (failed creating event source)",
            "desktop_input_source_unavailable:",
        ),
        (
            "primary monitor unavailable",
            "desktop_monitor_unavailable:",
        ),
        (
            "selected monitor is unavailable",
            "desktop_monitor_unavailable:",
        ),
        ("interactive desktop changed", "desktop_session_changed:"),
    ] {
        for suffix in ["", ": /private/secret"] {
            let response = error_response(&TaskServiceError::WindowHelper(
                crate::local_ipc::LocalIpcError::Remote(format!("{backend}{suffix}")),
            ));
            let DeviceTaskResponse::Error { code, message } = response else {
                panic!("expected error");
            };
            assert_eq!(code, DeviceTaskErrorCode::Unsupported);
            assert_eq!(message.starts_with(public), suffix.is_empty());
            assert!(!message.contains("/private/secret"));
        }
    }
}

fn account(id: u128) -> OperatorRef {
    OperatorRef::account(
        UserId::from_u128(id),
        pab_protocol::EndpointKey::new([id as u8; 32]),
    )
}

#[tokio::test]
async fn publication_record_wins_over_receipt_failure_and_cancel() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskStore::open(&directory.path().join("db")).await.unwrap();
    let id = RequestId::new();
    let actor = account(9);
    store
        .start_transfer(
            id,
            actor,
            "receive",
            "/test/result",
            3,
            Some(&"a".repeat(64)),
        )
        .await
        .unwrap();
    assert!(store.begin_transfer_publication(id).await.is_err());
    store.transfer_progress(id, 3, 3).await.unwrap();
    store.begin_transfer_publication(id).await.unwrap();
    store
        .finish_transfer(id, "failed", Some("lost receipt"))
        .await
        .unwrap();
    assert_eq!(
        store.get_transfer(actor, id).await.unwrap().state,
        "committing"
    );
    store.finish_transfer(id, "completed", None).await.unwrap();
    store
        .finish_transfer(id, "failed", Some("lost receipt"))
        .await
        .unwrap();
    let snapshot = store.get_transfer(actor, id).await.unwrap();
    assert_eq!(snapshot.state, "completed");
    assert_eq!(snapshot.published, Some(true));
}

#[tokio::test]
async fn publication_recovery_checks_actor_size_and_digest_without_replaying() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("db");
    let store = TaskStore::open(&database).await.unwrap();
    let id = RequestId::new();
    let actor = account(9);
    let destination = directory.path().join("中文 result.bin");
    let digest = format!("{:x}", Sha256::digest(b"data"));
    store
        .start_transfer(
            id,
            actor,
            "receive",
            destination.to_str().unwrap(),
            4,
            Some(&digest),
        )
        .await
        .unwrap();
    store.transfer_progress(id, 4, 4).await.unwrap();
    store.begin_transfer_publication(id).await.unwrap();
    tokio::fs::write(&destination, b"evil").await.unwrap();
    assert_eq!(
        store.get_transfer(actor, id).await.unwrap().state,
        "committing"
    );
    tokio::fs::write(&destination, b"data").await.unwrap();
    assert!(matches!(
        store.get_transfer(account(10), id).await,
        Err(TaskStoreError::NotFound)
    ));
    drop(store);
    let reopened = TaskStore::open(&database).await.unwrap();
    reopened.interrupt_transfers().await.unwrap();
    let snapshot = reopened.get_transfer(actor, id).await.unwrap();
    assert_eq!(snapshot.state, "completed");
    assert_eq!(snapshot.published, Some(true));
    assert_eq!(tokio::fs::read(destination).await.unwrap(), b"data");
}

#[tokio::test]
async fn a_proven_publication_failure_is_distinct_from_a_lost_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let store = TaskStore::open(&directory.path().join("db")).await.unwrap();
    let id = RequestId::new();
    let actor = account(9);
    store
        .start_transfer(
            id,
            actor,
            "receive",
            "/missing/result",
            0,
            Some(&"a".repeat(64)),
        )
        .await
        .unwrap();
    store.begin_transfer_publication(id).await.unwrap();
    store
        .publication_failed(id, "rename failed before publication")
        .await
        .unwrap();
    let snapshot = store.get_transfer(actor, id).await.unwrap();
    assert_eq!(snapshot.state, "failed");
    assert_eq!(snapshot.published, Some(false));
}

#[tokio::test]
async fn executor_transfer_audit_survives_reopen_and_records_interruption() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("tasks.sqlite3");
    let store = TaskStore::open(&database).await.unwrap();
    let operator = account(9);
    let completed = RequestId::from_u128(100);
    let interrupted = RequestId::from_u128(101);
    let checksum = "a".repeat(64);

    store
        .start_transfer(
            completed,
            operator,
            "receive",
            "/tmp/a.bin",
            512,
            Some(&checksum),
        )
        .await
        .unwrap();
    store.transfer_progress(completed, 512, 512).await.unwrap();
    store
        .finish_transfer(completed, "completed", None)
        .await
        .unwrap();
    let snapshot = store.get_transfer(operator, completed).await.unwrap();
    assert_eq!(snapshot.state, "completed");
    assert_eq!(snapshot.direction, "receive");
    assert_eq!(snapshot.offset, 512);
    assert_eq!(snapshot.sha256.as_deref(), Some(checksum.as_str()));
    let service = TaskService::open(
        &database,
        DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        },
        detect_native_execution_context().unwrap(),
    )
    .await
    .unwrap();
    let response = service
        .handle_request(
            operator,
            DeviceTaskRequest::GetTransfer {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id: completed,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        response,
        DeviceTaskResponse::Transfer { snapshot } if snapshot.request_id == completed
    ));
    assert!(matches!(
        store.get_transfer(account(10), completed).await,
        Err(TaskStoreError::NotFound)
    ));
    store
        .start_transfer(interrupted, operator, "send", "/tmp/b.bin", 0, None)
        .await
        .unwrap();
    store.transfer_hash(interrupted, &checksum).await.unwrap();
    assert_eq!(
        store
            .get_transfer(operator, interrupted)
            .await
            .unwrap()
            .sha256
            .as_deref(),
        Some(checksum.as_str())
    );
    drop(store);

    let reopened = TaskStore::open(&database).await.unwrap();
    reopened.interrupt_transfers().await.unwrap();
    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(&database);
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    let rows = sqlx::query_as::<_, (String, String, i64, i64)>(
        "SELECT request_id, state, offset, size FROM transfer_operations ORDER BY request_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        (completed.to_string(), "completed".to_owned(), 512, 512)
    );
    assert_eq!(rows[1].1, "interrupted");
}

#[tokio::test]
async fn executes_and_persists_a_command_with_live_output_ranges() {
    let directory = tempfile::tempdir().unwrap();
    let context = detect_native_execution_context().unwrap();
    let device_ref = DeviceRef {
        tenant_id: TenantId::from_u128(2),
        device_id: DeviceId::from_u128(3),
    };
    let service = TaskService::open(
        &directory.path().join("tasks.sqlite3"),
        device_ref,
        context.clone(),
    )
    .await
    .unwrap();
    let request_id = RequestId::from_u128(4);
    let command = test_command(&context);
    let accepted = service
        .submit_command(account(5), request_id, command.clone())
        .await
        .unwrap();
    let duplicate = service
        .submit_command(account(5), request_id, command)
        .await
        .unwrap();
    assert_eq!(accepted.task_ref, duplicate.task_ref);

    let snapshot = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = service
                .store
                .get_task(account(5), accepted.task_ref)
                .await
                .unwrap();
            if snapshot.state.is_terminal() {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("command did not finish");
    assert_eq!(snapshot.state, TaskState::Succeeded);
    assert!(snapshot.output.stdout.complete);
    assert!(snapshot.output.stderr.complete);

    let (stdout, _) = service
        .store
        .read_output(
            account(5),
            accepted.task_ref,
            OutputStream::Stdout,
            0,
            MAX_OUTPUT_READ_BYTES,
        )
        .await
        .unwrap();
    let (stderr, _) = service
        .store
        .read_output(
            account(5),
            accepted.task_ref,
            OutputStream::Stderr,
            0,
            MAX_OUTPUT_READ_BYTES,
        )
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&stdout.bytes).contains("pab-stdout"));
    assert!(String::from_utf8_lossy(&stderr.bytes).contains("pab-stderr"));
}

#[tokio::test]
async fn reopening_marks_an_accepted_task_interrupted() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory
        .path()
        .join("persistent-data")
        .join("tasks.sqlite3");
    let context = detect_native_execution_context().unwrap();
    let device_ref = DeviceRef {
        tenant_id: TenantId::from_u128(12),
        device_id: DeviceId::from_u128(13),
    };
    let initiated_by = account(14);
    let store = TaskStore::open(&database).await.unwrap();
    let accepted = store
        .accept_command(
            device_ref,
            initiated_by,
            RequestId::from_u128(15),
            &test_command(&context),
            context.clone(),
            1_000,
        )
        .await
        .unwrap();
    let task_ref = match accepted {
        AcceptTaskOutcome::Created(snapshot) | AcceptTaskOutcome::Existing(snapshot) => {
            snapshot.task_ref
        }
    };
    drop(store);

    let reopened = TaskService::open(&database, device_ref, context)
        .await
        .unwrap();
    let snapshot = reopened
        .store
        .get_task(initiated_by, task_ref)
        .await
        .unwrap();
    assert_eq!(snapshot.state, TaskState::Interrupted);
    assert_eq!(snapshot.latest_event_seq, 2);
    assert!(snapshot.output.stdout.complete);
    assert!(snapshot.output.stderr.complete);
}

#[tokio::test]
async fn active_task_limit_fails_new_work_but_preserves_request_deduplication() {
    let directory = tempfile::tempdir().unwrap();
    let context = detect_native_execution_context().unwrap();
    let device_ref = DeviceRef {
        tenant_id: TenantId::from_u128(22),
        device_id: DeviceId::from_u128(23),
    };
    let service = TaskService::open(
        &directory.path().join("tasks.sqlite3"),
        device_ref,
        context.clone(),
    )
    .await
    .unwrap();
    for value in 0..MAX_ACTIVE_TASKS {
        let (sender, _) = watch::channel(None);
        service
            .active
            .lock()
            .await
            .insert(TaskId::from_u128(value as u128 + 100), sender);
    }
    let initiated_by = account(24);
    let request_id = RequestId::from_u128(25);
    let command = test_command(&context);
    let rejected = service
        .submit_command(initiated_by, request_id, command.clone())
        .await
        .unwrap();
    assert_eq!(rejected.state, TaskState::Failed);
    assert_eq!(
        rejected.error.as_ref().map(|error| error.code.as_str()),
        Some("executor_busy")
    );
    assert!(rejected.output.stdout.complete);
    assert!(rejected.output.stderr.complete);

    let duplicate = service
        .submit_command(initiated_by, request_id, command)
        .await
        .unwrap();
    assert_eq!(duplicate.task_ref, rejected.task_ref);
    assert_eq!(duplicate.state, TaskState::Failed);
}

#[tokio::test]
async fn guest_task_history_is_isolated_by_endpoint_identity() {
    let directory = tempfile::tempdir().unwrap();
    let context = detect_native_execution_context().unwrap();
    let device_ref = DeviceRef {
        tenant_id: TenantId::from_u128(32),
        device_id: DeviceId::from_u128(33),
    };
    let store = TaskStore::open(&directory.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    let first_guest = pab_protocol::OperatorRef::guest(pab_protocol::EndpointKey::new([1; 32]));
    let other_guest = pab_protocol::OperatorRef::guest(pab_protocol::EndpointKey::new([2; 32]));
    let request_id = RequestId::from_u128(34);
    let command = test_command(&context);
    let first = store
        .accept_command(
            device_ref,
            first_guest,
            request_id,
            &command,
            context.clone(),
            1_000,
        )
        .await
        .unwrap();
    let first_ref = match first {
        AcceptTaskOutcome::Created(snapshot) => snapshot.task_ref,
        AcceptTaskOutcome::Existing(_) => panic!("first task was not created"),
    };
    assert!(store.get_task(first_guest, first_ref).await.is_ok());
    assert!(matches!(
        store.get_task(other_guest, first_ref).await,
        Err(TaskStoreError::NotFound)
    ));
    assert!(matches!(
        store.get_task(account(35), first_ref).await,
        Err(TaskStoreError::NotFound)
    ));
    let second = store
        .accept_command(
            device_ref,
            other_guest,
            request_id,
            &command,
            context,
            1_001,
        )
        .await
        .unwrap();
    assert!(matches!(second, AcceptTaskOutcome::Created(_)));
}

#[tokio::test]
async fn one_accounts_endpoints_have_separate_task_history() {
    let directory = tempfile::tempdir().unwrap();
    let context = detect_native_execution_context().unwrap();
    let device_ref = DeviceRef {
        tenant_id: TenantId::from_u128(42),
        device_id: DeviceId::from_u128(43),
    };
    let store = TaskStore::open(&directory.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    let user_id = UserId::from_u128(44);
    let first = OperatorRef::account(user_id, pab_protocol::EndpointKey::new([1; 32]));
    let second = OperatorRef::account(user_id, pab_protocol::EndpointKey::new([2; 32]));
    let request_id = RequestId::from_u128(45);
    let command = test_command(&context);
    let accepted = store
        .accept_command(
            device_ref,
            first,
            request_id,
            &command,
            context.clone(),
            1_000,
        )
        .await
        .unwrap();
    let first_ref = match accepted {
        AcceptTaskOutcome::Created(snapshot) => snapshot.task_ref,
        AcceptTaskOutcome::Existing(_) => panic!("first account endpoint did not create a task"),
    };
    assert!(store.get_task(first, first_ref).await.is_ok());
    assert!(matches!(
        store.get_task(second, first_ref).await,
        Err(TaskStoreError::NotFound)
    ));
    assert!(matches!(
        store
            .read_output(second, first_ref, OutputStream::Stdout, 0, 64)
            .await,
        Err(TaskStoreError::NotFound)
    ));
    let accepted = store
        .accept_command(device_ref, second, request_id, &command, context, 1_001)
        .await
        .unwrap();
    let second_ref = match accepted {
        AcceptTaskOutcome::Created(snapshot) => snapshot.task_ref,
        AcceptTaskOutcome::Existing(_) => panic!("second account endpoint reused the first task"),
    };
    assert_ne!(first_ref, second_ref);
}

fn test_command(context: &ExecutionContext) -> CommandTaskSpec {
    #[cfg(windows)]
    let (program, args) = (
        "cmd.exe".to_owned(),
        vec![
            "/D".to_owned(),
            "/S".to_owned(),
            "/C".to_owned(),
            "echo pab-stdout&& echo pab-stderr 1>&2".to_owned(),
        ],
    );
    #[cfg(not(windows))]
    let (program, args) = (
        "/bin/sh".to_owned(),
        vec![
            "-c".to_owned(),
            "printf 'pab-stdout\\n'; printf 'pab-stderr\\n' >&2".to_owned(),
        ],
    );
    CommandTaskSpec {
        options: Default::default(),
        program,
        args,
        cwd: None,
        expected_environment: ExpectedEnvironment {
            os_family: context.os_family,
            environment_revision: context.environment_revision.clone(),
        },
        display_summary: "task service integration test".to_owned(),
    }
}

#[tokio::test]
async fn output_reads_match_ranges_during_concurrent_append_and_trim() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("output-race.sqlite3");
    let reader = TaskStore::open(&path).await.unwrap();
    // Independent pools exercise the same WAL concurrency as separate processes.
    let writer = TaskStore::open(&path).await.unwrap();
    let context = detect_native_execution_context().unwrap();
    let actor = account(9);
    let device = DeviceRef {
        tenant_id: TenantId::new(),
        device_id: DeviceId::new(),
    };
    let accepted = writer
        .accept_command(
            device,
            actor,
            RequestId::new(),
            &test_command(&context),
            context,
            1_000,
        )
        .await
        .unwrap();
    let AcceptTaskOutcome::Created(snapshot) = accepted else {
        panic!("new task")
    };
    let task = snapshot.task_ref;
    writer
        .record_event(task, TaskEventKind::Running, 1_001)
        .await
        .unwrap();
    let append = async {
        for i in 0..128u64 {
            let bytes: Vec<_> = (i * 257..(i + 1) * 257).map(|n| (n % 251) as u8).collect();
            writer
                .append_output(task, OutputStream::Stdout, &bytes)
                .await
                .unwrap();
            tokio::task::yield_now().await;
        }
    };
    let read = async {
        for _ in 0..128 {
            let (chunk, range) = reader
                .read_output(actor, task, OutputStream::Stdout, 0, 65536)
                .await
                .unwrap();
            assert!(
                chunk.offset + chunk.bytes.len() as u64 <= range.available_to,
                "bytes and declared range must describe one snapshot"
            );
            assert_eq!(chunk.bytes.len() as u64, range.available_to);
            assert!(
                chunk
                    .bytes
                    .iter()
                    .enumerate()
                    .all(|(i, byte)| *byte == (i as u64 % 251) as u8)
            );
        }
    };
    tokio::join!(append, read);

    // Cross the retention boundary, then race another trim against a read.
    let start = 128 * 257u64;
    let end = 16 * 1024 * 1024u64 + 100;
    let bytes: Vec<_> = (start..end).map(|n| (n % 251) as u8).collect();
    writer
        .append_output(task, OutputStream::Stdout, &bytes)
        .await
        .unwrap();
    let bytes: Vec<_> = (end..end + 257).map(|n| (n % 251) as u8).collect();
    let (written, read) = tokio::join!(
        writer.append_output(task, OutputStream::Stdout, &bytes),
        reader.read_output(actor, task, OutputStream::Stdout, 100, 4096)
    );
    assert_eq!(written.unwrap().retained_from, 357);
    match read {
        Ok((chunk, range)) => {
            assert_eq!(range.retained_from, 100);
            assert!(
                chunk
                    .bytes
                    .iter()
                    .enumerate()
                    .all(|(i, byte)| *byte == ((100 + i as u64) % 251) as u8)
            );
        }
        Err(TaskStoreError::InvalidOutputOffset) => {} // trim committed before the read
        Err(error) => panic!("unexpected read failure: {error}"),
    }
    writer
        .complete_output(task, OutputStream::Stdout)
        .await
        .unwrap();
    let (chunk, range) = reader
        .read_output(actor, task, OutputStream::Stdout, end + 257, 10)
        .await
        .unwrap();
    assert!(range.complete);
    assert!(chunk.bytes.is_empty());
    assert!(matches!(
        reader
            .read_output(account(10), task, OutputStream::Stdout, 357, 10)
            .await,
        Err(TaskStoreError::NotFound)
    ));
}

#[tokio::test]
async fn command_options_deliver_stdin_env_deduplicate_and_stop_at_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let context = detect_native_execution_context().unwrap();
    let svc = TaskService::open(
        &dir.path().join("tasks.db"),
        DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        },
        context.clone(),
    )
    .await
    .unwrap();
    let mut command = test_command(&context);
    let marker = dir.path().join("executions.txt");
    command
        .options
        .env
        .insert("PAB_TEST_VALUE".into(), "中文".into());
    command.options.env.insert(
        "PAB_TEST_MARKER".into(),
        marker.to_string_lossy().into_owned(),
    );
    command.options.stdin_text = Some("stdin 😀".into());
    command.options.timeout_ms = Some(15000);
    #[cfg(windows)]
    {
        command.program = "powershell.exe".into();
        command.args=vec!["-NoProfile".into(),"-NonInteractive".into(),"-Command".into(),"[Console]::InputEncoding=[Text.UTF8Encoding]::new($false);[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false);[IO.File]::AppendAllText($env:PAB_TEST_MARKER,'x');[Console]::Out.Write($env:PAB_TEST_VALUE+':'+[Console]::In.ReadToEnd())".into()];
    }
    #[cfg(not(windows))]
    {
        command.program = "/bin/sh".into();
        command.args = vec![
            "-c".into(),
            "printf x >> \"$PAB_TEST_MARKER\"; printf '%s:' \"$PAB_TEST_VALUE\"; cat".into(),
        ];
    }
    let id = RequestId::new();
    let first = svc
        .submit_command(account(5), id, command.clone())
        .await
        .unwrap();
    assert_eq!(
        svc.submit_command(account(5), id, command.clone())
            .await
            .unwrap()
            .task_ref,
        first.task_ref
    );
    let completed = wait_command(&svc, first.task_ref).await;
    assert_eq!(
        completed.state,
        TaskState::Succeeded,
        "{:?}",
        completed.error
    );
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "x");
    let (output, _) = svc
        .store
        .read_output(account(5), first.task_ref, OutputStream::Stdout, 0, 8192)
        .await
        .unwrap();
    assert_eq!(String::from_utf8(output.bytes).unwrap(), "中文:stdin 😀");
    command.options.stdin_text = Some("different".into());
    assert!(
        svc.submit_command(account(5), id, command.clone())
            .await
            .is_err()
    );
    command.options = pab_protocol::CommandOptions {
        timeout_ms: Some(50),
        ..Default::default()
    };
    #[cfg(windows)]
    {
        command.args = vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            "Start-Sleep -Seconds 10".into(),
        ];
    }
    #[cfg(not(windows))]
    {
        command.program = "/bin/sleep".into();
        command.args = vec!["10".into()];
    }
    let task = svc
        .submit_command(account(5), RequestId::new(), command)
        .await
        .unwrap();
    let completed = wait_command(&svc, task.task_ref).await;
    assert_eq!(completed.state, TaskState::Failed);
    assert_eq!(completed.error.unwrap().code, "execution_timeout");
}

async fn wait_command(svc: &TaskService, task: TaskRef) -> TaskSnapshot {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let record = svc.store.get_task(account(5), task).await.unwrap();
            if record.state.is_terminal() {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
