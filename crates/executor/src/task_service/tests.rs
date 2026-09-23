use std::time::Duration;

use pab_platform::detect_native_execution_context;
use pab_protocol::{
    DeploymentId, DeviceId, ExpectedEnvironment, RequestId, TaskState, TenantId, UserId,
};

use super::*;

#[tokio::test]
async fn executor_transfer_audit_survives_reopen_and_records_interruption() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("tasks.sqlite3");
    let store = TaskStore::open(&database).await.unwrap();
    let operator = OperatorRef::Account(UserId::from_u128(9));
    let completed = RequestId::from_u128(100);
    let interrupted = RequestId::from_u128(101);

    store
        .start_transfer(completed, operator, "receive", "/tmp/a.bin", 512)
        .await
        .unwrap();
    store.transfer_progress(completed, 512, 512).await.unwrap();
    store
        .finish_transfer(completed, "completed", None)
        .await
        .unwrap();
    store
        .start_transfer(interrupted, operator, "send", "/tmp/b.bin", 0)
        .await
        .unwrap();
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
        deployment_id: DeploymentId::from_u128(1),
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
        .submit_command(UserId::from_u128(5).into(), request_id, command.clone())
        .await
        .unwrap();
    let duplicate = service
        .submit_command(UserId::from_u128(5).into(), request_id, command)
        .await
        .unwrap();
    assert_eq!(accepted.task_ref, duplicate.task_ref);

    let snapshot = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = service
                .store
                .get_task(UserId::from_u128(5).into(), accepted.task_ref)
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
            UserId::from_u128(5).into(),
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
            UserId::from_u128(5).into(),
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
        deployment_id: DeploymentId::from_u128(11),
        tenant_id: TenantId::from_u128(12),
        device_id: DeviceId::from_u128(13),
    };
    let initiated_by = UserId::from_u128(14).into();
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
        deployment_id: DeploymentId::from_u128(21),
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
    let initiated_by = UserId::from_u128(24).into();
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
        deployment_id: DeploymentId::from_u128(31),
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
        store
            .get_task(UserId::from_u128(35).into(), first_ref)
            .await,
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
