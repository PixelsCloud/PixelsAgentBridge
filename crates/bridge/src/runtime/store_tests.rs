use pab_protocol::{
    CapabilityRef, CommandTaskSpec, CpuArchitecture, DeploymentId, DeviceId, DeviceRef,
    ExecutionContext, ExecutionScope, ExpectedEnvironment, OsFamily, OutputAvailability,
    OutputChunk, OutputRange, OutputStream, PathStyle, RequestId, TASK_SCHEMA_VERSION,
    TaskCompletion, TaskEvent, TaskEventKind, TaskId, TaskRef, TaskSnapshot, TaskState, TenantId,
    UserId,
};
use tempfile::tempdir;

use super::store::*;

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
        initiated_by: UserId::from_u128(6),
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
