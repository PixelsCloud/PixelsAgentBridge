use pab_protocol::{
    CapabilityRef, CommandTaskSpec, CpuArchitecture, DeploymentId, DeviceCode, DeviceId, DeviceRef,
    ExecutionContext, ExecutionScope, ExpectedEnvironment, OperatorRef, OsFamily,
    OutputAvailability, OutputChunk, OutputRange, OutputStream, PathStyle, RequestId,
    TASK_SCHEMA_VERSION, TaskCompletion, TaskEvent, TaskEventKind, TaskId, TaskRef, TaskSnapshot,
    TaskState, TenantId, UserId,
};
use tempfile::tempdir;

use super::RememberedDevice;
use super::store::*;
use super::{BridgeLocalStore, DevicePasswordProvider, SqliteDevicePasswordProvider};

#[tokio::test]
async fn concurrent_stores_initialize_one_database() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("bridge.sqlite3");
    let (first, second) = tokio::join!(RuntimeStore::open(&path), RuntimeStore::open(&path));
    let first = first.unwrap();
    let second = second.unwrap();
    first.start_session("first").await.unwrap();
    second.start_session("second").await.unwrap();
    assert!(first.operations().await.unwrap().is_empty());
}

#[tokio::test]
async fn reporting_operations_include_only_the_owning_runtime() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("bridge.sqlite3");
    let first = RuntimeStore::open(&path).await.unwrap();
    let second = RuntimeStore::open(&path).await.unwrap();
    first.start_session("mcp-first").await.unwrap();
    second.start_session("mcp-second").await.unwrap();
    for (id, owner) in [
        (RequestId::new(), "mcp-first"),
        (RequestId::new(), "mcp-second"),
    ] {
        first
            .start_operation(
                id,
                device_ref(),
                None,
                "guest",
                "upload",
                "local.bin",
                "/tmp/remote.bin",
                false,
                Some(owner),
            )
            .await
            .unwrap();
    }
    let first_records = first.operations_for_session("mcp-first").await.unwrap();
    let second_records = second.operations_for_session("mcp-second").await.unwrap();
    assert_eq!(first_records.len(), 1);
    assert_eq!(second_records.len(), 1);
    assert_ne!(first_records[0].id, second_records[0].id);
    assert!(
        first
            .operations_for_session("desktop")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn device_password_is_available_to_another_process_store() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("bridge.sqlite3");
    let local = BridgeLocalStore::open(&path).await.unwrap();
    local
        .save_device_password(device_ref().device_id, "TEMP1234")
        .await
        .unwrap();

    let provider = SqliteDevicePasswordProvider::new(&path);
    let password = provider.load_password(device_ref()).await.unwrap();
    assert_eq!(password.as_str(), "TEMP1234");

    local
        .save_device_password(device_ref().device_id, "NEWPASS8")
        .await
        .unwrap();
    let updated = provider.load_password(device_ref()).await.unwrap();
    assert_eq!(updated.as_str(), "NEWPASS8");
}

#[tokio::test]
async fn remembers_device_name_across_reconnect_and_reopen() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("bridge.sqlite3");
    let store = RuntimeStore::open(&path).await.unwrap();
    let mut device = RememberedDevice {
        device_ref: device_ref(),
        code: DeviceCode::new(123_456_789).unwrap(),
        alias: String::new(),
        os_family: OsFamily::Linux,
        os_reminder: "Linux shell".to_owned(),
    };
    store.remember_device(&device).await.unwrap();
    store.rename_device(device.code, "SG Linux").await.unwrap();
    let other = RememberedDevice {
        device_ref: DeviceRef {
            device_id: DeviceId::from_u128(4),
            ..device_ref()
        },
        code: DeviceCode::new(987_654_321).unwrap(),
        alias: "Windows 90".to_owned(),
        os_family: OsFamily::Windows,
        os_reminder: "Windows PowerShell".to_owned(),
    };
    store.remember_device(&other).await.unwrap();
    device.os_reminder = "Linux bash".to_owned();
    store.remember_device(&device).await.unwrap();
    store.close().await;

    let reopened = RuntimeStore::open(&path).await.unwrap();
    let remembered = reopened.remembered_devices().await.unwrap();
    assert_eq!(remembered.len(), 2);
    assert_eq!(remembered[0].alias, "SG Linux");
    assert_eq!(remembered[0].os_reminder, "Linux bash");
    assert_eq!(remembered[0].device_ref, device.device_ref);
    assert_eq!(remembered[1].device_ref, other.device_ref);
}

#[tokio::test]
async fn forgetting_device_removes_saved_password() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("bridge.sqlite3");
    let local = BridgeLocalStore::open(&path).await.unwrap();
    let code = DeviceCode::new(123_456_789).unwrap();
    let device = RememberedDevice {
        device_ref: device_ref(),
        code,
        alias: "Test device".to_owned(),
        os_family: OsFamily::Linux,
        os_reminder: "Linux shell".to_owned(),
    };
    local.store.remember_device(&device).await.unwrap();
    local
        .save_device_password(device.device_ref.device_id, "PASSWORD")
        .await
        .unwrap();

    let removed_id = local.forget_device(code).await.unwrap();
    assert_eq!(removed_id, device.device_ref.device_id);
    assert!(local.remembered_devices().await.unwrap().is_empty());
    let saved_password: Option<String> =
        sqlx::query_scalar("SELECT password FROM device_credentials WHERE device_id = ?")
            .bind(removed_id.to_string())
            .fetch_optional(&local.store.pool)
            .await
            .unwrap();
    assert!(saved_password.is_none());
}

#[tokio::test]
async fn transfer_history_survives_reopen_without_changing_active_transfer() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("bridge.sqlite3");
    let store = RuntimeStore::open(&path).await.unwrap();
    let completed = RequestId::from_u128(100);
    let interrupted = RequestId::from_u128(101);

    store
        .start_operation(
            completed,
            device_ref(),
            Some(DeviceCode::new(123_456_789).unwrap()),
            "guest",
            "upload",
            "a.bin",
            "/tmp/a.bin",
            false,
            None,
        )
        .await
        .unwrap();
    store.operation_progress(completed, 512, 512).await.unwrap();
    store
        .finish_operation(completed, "completed", None)
        .await
        .unwrap();
    store
        .start_operation(
            interrupted,
            device_ref(),
            None,
            "guest",
            "download",
            "/tmp/b.bin",
            "b.bin",
            true,
            None,
        )
        .await
        .unwrap();
    store.close().await;

    let reopened = RuntimeStore::open(&path).await.unwrap();
    let operations = reopened.operations().await.unwrap();
    assert_eq!(operations.len(), 2);
    let upload = operations
        .iter()
        .find(|item| item.id == completed.to_string())
        .unwrap();
    assert_eq!(upload.state, "completed");
    assert_eq!(upload.offset, 512);
    assert_eq!(upload.size, 512);
    assert_eq!(upload.direction, "upload");
    assert_eq!(
        upload.device_code,
        Some(DeviceCode::new(123_456_789).unwrap())
    );
    assert_eq!(upload.initiated_by, "guest");
    let download = operations
        .iter()
        .find(|item| item.id == interrupted.to_string())
        .unwrap();
    assert_eq!(download.state, "running");
    assert!(download.finished_at_unix_ms.is_none());
}

#[tokio::test]
async fn shared_store_observes_each_runtime_session_without_rewriting_transfer_state() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("bridge.sqlite3");
    let first = RuntimeStore::open(&path).await.unwrap();
    let second = RuntimeStore::open(&path).await.unwrap();
    first.start_session("first").await.unwrap();
    second.start_session("second").await.unwrap();

    let first_id = RequestId::from_u128(201);
    let second_id = RequestId::from_u128(202);
    first
        .start_operation(
            first_id,
            device_ref(),
            None,
            "guest",
            "upload",
            "first.bin",
            "/tmp/first.bin",
            false,
            Some("first"),
        )
        .await
        .unwrap();
    second
        .start_operation(
            second_id,
            device_ref(),
            None,
            "guest",
            "upload",
            "second.bin",
            "/tmp/second.bin",
            false,
            Some("second"),
        )
        .await
        .unwrap();

    first.stop_session("first").await.unwrap();
    let operations = second.operations().await.unwrap();
    let stopped = operations
        .iter()
        .find(|item| item.id == first_id.to_string())
        .unwrap();
    let active = operations
        .iter()
        .find(|item| item.id == second_id.to_string())
        .unwrap();
    assert_eq!(stopped.state, "running");
    assert_eq!(
        stopped.execution_observation.as_deref(),
        Some("unconfirmed")
    );
    assert_eq!(active.state, "running");
    assert_eq!(active.execution_observation.as_deref(), Some("active"));

    assert!(
        second
            .finish_operation(second_id, "completed", None)
            .await
            .unwrap()
    );
    assert!(
        !second
            .finish_operation(second_id, "cancelled", None)
            .await
            .unwrap()
    );
    let completed = second
        .operations()
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.id == second_id.to_string())
        .unwrap();
    assert_eq!(completed.state, "completed");
    assert_eq!(completed.execution_observation, None);
}

#[tokio::test]
async fn stale_heartbeat_reports_unconfirmed_without_failing_transfer() {
    let directory = tempdir().unwrap();
    let store = RuntimeStore::open(&directory.path().join("bridge.sqlite3"))
        .await
        .unwrap();
    store.start_session("stale").await.unwrap();
    let id = RequestId::from_u128(203);
    store
        .start_operation(
            id,
            device_ref(),
            None,
            "guest",
            "download",
            "/tmp/source.bin",
            "local.bin",
            false,
            Some("stale"),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE runtime_sessions SET heartbeat_at_unix_ms = 1 WHERE id = 'stale'")
        .execute(&store.pool)
        .await
        .unwrap();

    let operation = store.operations().await.unwrap().pop().unwrap();
    assert_eq!(operation.state, "running");
    assert_eq!(
        operation.execution_observation.as_deref(),
        Some("unconfirmed")
    );
    assert!(operation.finished_at_unix_ms.is_none());
}

#[tokio::test]
async fn cancellation_keeps_remote_result_unconfirmed_until_reconciled() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("bridge.sqlite3");
    let first = RuntimeStore::open(&path).await.unwrap();
    let second = RuntimeStore::open(&path).await.unwrap();
    first.start_session("operator").await.unwrap();
    second.start_session("other").await.unwrap();
    let id = RequestId::from_u128(204);
    first
        .start_operation(
            id,
            device_ref(),
            None,
            "guest",
            "upload",
            "local.bin",
            "/tmp/remote.bin",
            false,
            Some("operator"),
        )
        .await
        .unwrap();

    assert!(
        !second
            .request_transfer_cancellation(id, "other")
            .await
            .unwrap()
    );
    assert!(
        first
            .request_transfer_cancellation(id, "operator")
            .await
            .unwrap()
    );
    assert!(
        !first
            .request_transfer_cancellation(id, "operator")
            .await
            .unwrap()
    );
    second
        .operation_progress(id, 65_536, 4_194_304)
        .await
        .unwrap();
    let operation = second.operations().await.unwrap().pop().unwrap();
    assert_eq!(operation.state, "cancel_requested");
    assert_eq!(operation.offset, 65_536);
    assert_eq!(operation.size, 4_194_304);
    assert_eq!(
        operation.execution_observation.as_deref(),
        Some("unconfirmed")
    );
    assert!(operation.finished_at_unix_ms.is_none());
    assert_eq!(
        second.cancellation_requests(8, None).await.unwrap().len(),
        1
    );

    assert!(
        second
            .finish_operation(id, "completed", None)
            .await
            .unwrap()
    );
    assert!(
        second
            .cancellation_requests(8, None)
            .await
            .unwrap()
            .is_empty()
    );
    let operation = first.operations().await.unwrap().pop().unwrap();
    assert_eq!(operation.state, "completed");
    assert!(operation.finished_at_unix_ms.is_some());
}

#[tokio::test]
async fn prepared_transfer_record_is_idempotent_only_for_the_same_operation() {
    let directory = tempdir().unwrap();
    let store = RuntimeStore::open(&directory.path().join("bridge.sqlite3"))
        .await
        .unwrap();
    store.start_session("operator").await.unwrap();
    let id = RequestId::from_u128(205);
    for _ in 0..2 {
        store
            .start_operation(
                id,
                device_ref(),
                None,
                "guest",
                "upload",
                "local.bin",
                "/tmp/remote.bin",
                false,
                Some("operator"),
            )
            .await
            .unwrap();
    }
    let conflict = store
        .start_operation(
            id,
            device_ref(),
            None,
            "guest",
            "upload",
            "different.bin",
            "/tmp/remote.bin",
            false,
            Some("operator"),
        )
        .await
        .unwrap_err();
    assert!(matches!(conflict, RuntimeStoreError::RequestConflict));
    assert!(
        store
            .request_transfer_cancellation(id, "operator")
            .await
            .unwrap()
    );
    store
        .start_operation(
            id,
            device_ref(),
            None,
            "guest",
            "upload",
            "local.bin",
            "/tmp/remote.bin",
            false,
            Some("operator"),
        )
        .await
        .unwrap();
    assert_eq!(store.operations().await.unwrap().len(), 1);
}

#[tokio::test]
async fn unconfirmed_transfer_lookup_pages_without_repeating_a_record() {
    let directory = tempdir().unwrap();
    let store = RuntimeStore::open(&directory.path().join("bridge.sqlite3"))
        .await
        .unwrap();
    store.start_session("former").await.unwrap();
    for value in 301..=303 {
        store
            .start_operation(
                RequestId::from_u128(value),
                device_ref(),
                None,
                "guest",
                "upload",
                "source.bin",
                "/tmp/destination.bin",
                false,
                Some("former"),
            )
            .await
            .unwrap();
    }
    store.stop_session("former").await.unwrap();

    let first = store.unconfirmed_transfers(2, None).await.unwrap();
    assert_eq!(first.len(), 2);
    let last = first.last().unwrap();
    let second = store
        .unconfirmed_transfers(2, Some((last.started_at_unix_ms, &last.id)))
        .await
        .unwrap();
    assert_eq!(second.len(), 1);
    assert!(first.iter().all(|record| record.id != second[0].id));

    let first_history = store.operations_page(None, 2).await.unwrap();
    let history_cursor = first_history.last().unwrap();
    let second_history = store
        .operations_page(
            Some((history_cursor.started_at_unix_ms, &history_cursor.id)),
            2,
        )
        .await
        .unwrap();
    assert_eq!(second_history.len(), 1);
    assert!(
        first_history
            .iter()
            .all(|record| record.id != second_history[0].id)
    );
}

#[tokio::test]
async fn task_history_pages_only_accepted_tasks() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("bridge.sqlite3");
    let store = RuntimeStore::open(&path).await.unwrap();
    for value in 400..403 {
        let request_id = RequestId::from_u128(value);
        store
            .record_pending(device_ref(), request_id, &command())
            .await
            .unwrap();
        if value != 401 {
            let mut accepted = snapshot(device_ref(), request_id);
            accepted.task_ref.task_id = TaskId::from_u128(value);
            store.bind_snapshot(&accepted).await.unwrap();
        }
    }

    let first = store.list_page(None, 1).await.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].request_id, RequestId::from_u128(402));
    let second = store.list_page(Some(first[0].request_id), 1).await.unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].request_id, RequestId::from_u128(400));
    assert!(
        store
            .list_page(Some(second[0].request_id), 1)
            .await
            .unwrap()
            .is_empty()
    );
    store.close().await;

    let reopened = RuntimeStore::open(&path).await.unwrap();
    let saved = reopened
        .get_by_task_id(TaskId::from_u128(400))
        .await
        .unwrap();
    assert_eq!(saved.request_id, RequestId::from_u128(400));
    assert_eq!(
        saved.snapshot.unwrap().task_ref.task_id,
        TaskId::from_u128(400)
    );
}

#[tokio::test]
async fn history_count_includes_all_pages_and_filters_by_device() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("bridge.sqlite3");
    let store = RuntimeStore::open(&path).await.unwrap();
    let first_device = device_ref();
    let second_device = DeviceRef {
        device_id: DeviceId::from_u128(44),
        ..first_device
    };
    for (value, device_ref) in [(1, first_device), (2, second_device), (3, first_device)] {
        let request_id = RequestId::from_u128(value);
        store
            .record_pending(device_ref, request_id, &command())
            .await
            .unwrap();
        if value != 2 {
            let mut accepted = snapshot(device_ref, request_id);
            accepted.task_ref.task_id = TaskId::from_u128(value);
            store.bind_snapshot(&accepted).await.unwrap();
        }
        store
            .start_operation(
                RequestId::from_u128(value + 100),
                device_ref,
                None,
                "guest",
                "upload",
                "source",
                "destination",
                false,
                None,
            )
            .await
            .unwrap();
    }
    let local = BridgeLocalStore::open(&path).await.unwrap();
    assert_eq!(local.history_count(None).await.unwrap(), 5);
    assert_eq!(local.history_count(Some(first_device)).await.unwrap(), 4);
    assert_eq!(local.history_count(Some(second_device)).await.unwrap(), 1);
    assert_eq!(local.tasks_page(None, 1).await.unwrap().len(), 1);
    assert_eq!(local.history_count(None).await.unwrap(), 5);
}

#[tokio::test]
async fn device_history_pages_exclude_other_devices() {
    let directory = tempdir().unwrap();
    let store = RuntimeStore::open(&directory.path().join("bridge.sqlite3"))
        .await
        .unwrap();
    let first_device = device_ref();
    let second_device = DeviceRef {
        device_id: DeviceId::from_u128(44),
        ..first_device
    };

    for (value, device_ref) in [(1, first_device), (2, second_device), (3, first_device)] {
        let request_id = RequestId::from_u128(value);
        store
            .record_pending(device_ref, request_id, &command())
            .await
            .unwrap();
        let mut accepted = snapshot(device_ref, request_id);
        accepted.task_ref.task_id = TaskId::from_u128(value);
        store.bind_snapshot(&accepted).await.unwrap();
        store
            .start_operation(
                RequestId::from_u128(value + 100),
                device_ref,
                None,
                "guest",
                "upload",
                "source",
                "destination",
                false,
                None,
            )
            .await
            .unwrap();
    }

    let first_tasks = store
        .list_page_for_device(first_device, None, 1)
        .await
        .unwrap();
    assert_eq!(first_tasks[0].request_id, RequestId::from_u128(3));
    let next_tasks = store
        .list_page_for_device(first_device, Some(first_tasks[0].request_id), 1)
        .await
        .unwrap();
    assert_eq!(next_tasks[0].request_id, RequestId::from_u128(1));

    let first_operations = store
        .operations_page_for_device(first_device, None, 1)
        .await
        .unwrap();
    let operation = &first_operations[0];
    assert_eq!(operation.device_ref, first_device);
    let next_operations = store
        .operations_page_for_device(
            first_device,
            Some((operation.started_at_unix_ms, &operation.id)),
            1,
        )
        .await
        .unwrap();
    assert_eq!(next_operations.len(), 1);
    assert_eq!(next_operations[0].device_ref, first_device);
    assert_ne!(next_operations[0].id, operation.id);
    assert_eq!(
        store
            .list_page_for_device(second_device, None, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn persists_submission_events_output_and_resume_cursor() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("persistent-data").join("bridge.db");
    let store = RuntimeStore::open(&path).await.unwrap();
    let device_ref = device_ref();
    let request_id = RequestId::from_u128(4);
    let command = command();

    let pending = store
        .record_pending(device_ref, request_id, &command)
        .await
        .unwrap();
    assert!(pending.snapshot.is_none());
    assert_eq!(store.incomplete().await.unwrap().len(), 1);

    let mut snapshot = snapshot(device_ref, request_id);
    store.bind_snapshot(&snapshot).await.unwrap();
    let accepted = TaskEvent {
        schema_version: TASK_SCHEMA_VERSION,
        task_ref: snapshot.task_ref,
        seq: 1,
        occurred_at_unix_ms: 1_000,
        kind: TaskEventKind::Accepted,
    };
    assert!(store.record_event(&accepted).await.unwrap());
    assert!(!store.record_event(&accepted).await.unwrap());

    let chunk = OutputChunk {
        schema_version: TASK_SCHEMA_VERSION,
        task_ref: snapshot.task_ref,
        stream: OutputStream::Stdout,
        offset: 0,
        bytes: b"hello".to_vec(),
    };
    assert!(
        store
            .append_output(
                &chunk,
                &OutputRange {
                    retained_from: 0,
                    available_to: 5,
                    complete: false,
                },
            )
            .await
            .unwrap()
    );

    let succeeded = TaskEvent {
        schema_version: TASK_SCHEMA_VERSION,
        task_ref: snapshot.task_ref,
        seq: 2,
        occurred_at_unix_ms: 2_000,
        kind: TaskEventKind::Succeeded {
            completion: TaskCompletion {
                summary: "done".to_owned(),
                exit_code: Some(0),
            },
        },
    };
    store.record_event(&succeeded).await.unwrap();
    snapshot.state = TaskState::Succeeded;
    snapshot.latest_event_seq = 2;
    snapshot.finished_at_unix_ms = Some(2_000);
    snapshot.completion = Some(TaskCompletion {
        summary: "done".to_owned(),
        exit_code: Some(0),
    });
    snapshot.output.stdout = OutputRange {
        retained_from: 0,
        available_to: 5,
        complete: true,
    };
    snapshot.output.stderr.complete = true;
    let completed = store.update_snapshot(&snapshot).await.unwrap();
    assert!(completed.is_complete());

    let (output, range) = store
        .read_output(snapshot.task_ref, OutputStream::Stdout, 1, 3)
        .await
        .unwrap();
    assert_eq!(output.bytes, b"ell");
    assert!(range.complete);
    assert_eq!(
        store.events_after(snapshot.task_ref, 0).await.unwrap(),
        vec![accepted, succeeded]
    );

    store.close().await;
    let reopened = RuntimeStore::open(&path).await.unwrap();
    let restored = reopened.get_by_request(request_id).await.unwrap();
    assert!(restored.is_complete());
    assert!(reopened.incomplete().await.unwrap().is_empty());
}

#[tokio::test]
async fn rejects_request_id_reuse_with_different_command() {
    let directory = tempdir().unwrap();
    let store = RuntimeStore::open(&directory.path().join("bridge.db"))
        .await
        .unwrap();
    let request_id = RequestId::from_u128(4);
    store
        .record_pending(device_ref(), request_id, &command())
        .await
        .unwrap();
    let mut changed = command();
    changed.program = "different".to_owned();
    assert!(matches!(
        store
            .record_pending(device_ref(), request_id, &changed)
            .await,
        Err(RuntimeStoreError::RequestConflict)
    ));
}

#[tokio::test]
async fn adopts_a_remote_task_without_inventing_a_command() {
    let directory = tempdir().unwrap();
    let store = RuntimeStore::open(&directory.path().join("bridge.db"))
        .await
        .unwrap();
    let mut remote = snapshot(device_ref(), RequestId::from_u128(40));
    remote.output.stdout = OutputRange {
        retained_from: 100,
        available_to: 120,
        complete: false,
    };

    let adopted = store.adopt_snapshot(&remote).await.unwrap();

    assert!(adopted.command.is_none());
    assert_eq!(adopted.stdout.retained_from, 100);
    assert_eq!(adopted.stdout.available_to, 100);
    assert!(!adopted.is_complete());
}

#[tokio::test]
async fn advances_a_stale_cursor_to_the_remote_retention_boundary() {
    let directory = tempdir().unwrap();
    let store = RuntimeStore::open(&directory.path().join("bridge.db"))
        .await
        .unwrap();
    let request_id = RequestId::from_u128(41);
    store
        .record_pending(device_ref(), request_id, &command())
        .await
        .unwrap();
    let mut remote = snapshot(device_ref(), request_id);
    store.bind_snapshot(&remote).await.unwrap();
    remote.output.stdout = OutputRange {
        retained_from: 100,
        available_to: 120,
        complete: false,
    };

    let (record, gaps) = store.reconcile_snapshot(&remote).await.unwrap();

    assert_eq!(record.stdout.retained_from, 100);
    assert_eq!(record.stdout.available_to, 100);
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].stream, OutputStream::Stdout);
    assert_eq!(gaps[0].missing_from, 0);
    assert_eq!(gaps[0].missing_to, 100);
}

fn device_ref() -> DeviceRef {
    DeviceRef {
        deployment_id: DeploymentId::from_u128(1),
        tenant_id: TenantId::from_u128(2),
        device_id: DeviceId::from_u128(3),
    }
}

#[test]
fn runtime_reporting_tracks_task_events_without_forwarding_output_or_command_arguments() {
    let mut report = crate::desktop_presence::RuntimeReport::default();
    let mut task = snapshot(device_ref(), RequestId::new());
    task.display_summary = "program private-command-argument".to_owned();
    let target = task.target_context();
    super::presence::apply_event(
        &mut report,
        &super::RuntimeEvent {
            sequence: 1,
            occurred_at_unix_ms: 1000,
            kind: super::RuntimeEventKind::TaskSnapshot {
                target: target.clone(),
                snapshot: Box::new(task.clone()),
            },
        },
    );
    super::presence::apply_event(
        &mut report,
        &super::RuntimeEvent {
            sequence: 2,
            occurred_at_unix_ms: 2000,
            kind: super::RuntimeEventKind::TaskEvent {
                target: target.clone(),
                event: TaskEvent {
                    schema_version: TASK_SCHEMA_VERSION,
                    task_ref: task.task_ref,
                    seq: 2,
                    occurred_at_unix_ms: 2000,
                    kind: TaskEventKind::Running,
                },
            },
        },
    );
    assert_eq!(report.tasks[0].state, "running");
    super::presence::apply_event(
        &mut report,
        &super::RuntimeEvent {
            sequence: 3,
            occurred_at_unix_ms: 2001,
            kind: super::RuntimeEventKind::TaskOutput {
                target: target.clone(),
                chunk: OutputChunk {
                    schema_version: TASK_SCHEMA_VERSION,
                    task_ref: task.task_ref,
                    stream: OutputStream::Stdout,
                    offset: 0,
                    bytes: b"private-output".to_vec(),
                },
                range: OutputRange {
                    retained_from: 0,
                    available_to: 14,
                    complete: false,
                },
            },
        },
    );
    assert_eq!(report.tasks[0].output.stdout.available_to, 14);
    let serialized = serde_json::to_string(&report).unwrap();
    assert!(!serialized.contains("private-output"));
    assert!(!serialized.contains("private-command-argument"));
    super::presence::apply_event(
        &mut report,
        &super::RuntimeEvent {
            sequence: 4,
            occurred_at_unix_ms: 3000,
            kind: super::RuntimeEventKind::TaskEvent {
                target,
                event: TaskEvent {
                    schema_version: TASK_SCHEMA_VERSION,
                    task_ref: task.task_ref,
                    seq: 3,
                    occurred_at_unix_ms: 3000,
                    kind: TaskEventKind::Succeeded {
                        completion: TaskCompletion {
                            summary: "private-summary".to_owned(),
                            exit_code: Some(0),
                        },
                    },
                },
            },
        },
    );
    assert_eq!(report.tasks[0].state, "succeeded");
    assert_eq!(report.tasks[0].exit_code, Some(0));
    assert_eq!(report.tasks[0].finished_at_unix_ms, Some(3000));
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("private-summary")
    );
}

fn command() -> CommandTaskSpec {
    CommandTaskSpec {
        program: "printf".to_owned(),
        args: vec!["hello".to_owned()],
        cwd: Some("/tmp".to_owned()),
        expected_environment: ExpectedEnvironment {
            os_family: OsFamily::Linux,
            environment_revision: "linux-test".to_owned(),
        },
        display_summary: "printf …".to_owned(),
    }
}

fn snapshot(device_ref: DeviceRef, request_id: RequestId) -> TaskSnapshot {
    TaskSnapshot {
        schema_version: TASK_SCHEMA_VERSION,
        task_ref: TaskRef {
            device_ref,
            task_id: TaskId::from_u128(5),
        },
        request_id,
        initiated_by: OperatorRef::account(
            UserId::from_u128(6),
            pab_protocol::EndpointKey::new([6; 32]),
        ),
        capability: CapabilityRef {
            name: "process.exec".to_owned(),
            version: 1,
        },
        display_summary: "printf …".to_owned(),
        state: TaskState::Accepted,
        stage: None,
        latest_event_seq: 1,
        created_at_unix_ms: 1_000,
        started_at_unix_ms: None,
        finished_at_unix_ms: None,
        progress: None,
        completion: None,
        error: None,
        output: OutputAvailability::default(),
        execution_context: ExecutionContext {
            os_family: OsFamily::Linux,
            os_name: "Linux".to_owned(),
            os_version: "test".to_owned(),
            architecture: CpuArchitecture::X86_64,
            execution_scope: ExecutionScope::Native,
            path_style: PathStyle::Posix,
            interpreter: None,
            cwd: Some("/tmp".to_owned()),
            environment_revision: "linux-test".to_owned(),
        },
    }
}
