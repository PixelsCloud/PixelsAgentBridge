use pab_protocol::{
    CapabilityRef, CpuArchitecture, DeploymentId, DeviceId, DeviceRef, ExecutionContext,
    ExecutionScope, OsFamily, OutputStream, PathStyle, RequestId, TaskCompletion, TaskEventKind,
    TaskId, TaskProgress, TaskRef, TaskState, TenantId, TransferDirection, TransferPhase,
    TransferProgress, UserId,
};
use pab_task_runtime::{AcceptedTask, EventApply, TaskAggregate, TaskRuntimeError};

fn accepted(os_family: OsFamily, path_style: PathStyle) -> AcceptedTask {
    AcceptedTask {
        task_ref: TaskRef {
            device_ref: DeviceRef {
                deployment_id: DeploymentId::from_u128(1),
                tenant_id: TenantId::from_u128(2),
                device_id: DeviceId::from_u128(3),
            },
            task_id: TaskId::from_u128(4),
        },
        request_id: RequestId::from_u128(5),
        initiated_by: UserId::from_u128(6),
        capability: CapabilityRef {
            name: "file.transfer".to_owned(),
            version: 1,
        },
        display_summary: "Copy archive to target".to_owned(),
        execution_context: ExecutionContext {
            os_family,
            os_name: os_family.to_string(),
            os_version: "test".to_owned(),
            architecture: CpuArchitecture::X86_64,
            execution_scope: ExecutionScope::Native,
            path_style,
            interpreter: None,
            cwd: Some(match os_family {
                OsFamily::Windows => r"C:\Work".to_owned(),
                OsFamily::Linux | OsFamily::Macos => "/work".to_owned(),
            }),
            environment_revision: "env-7".to_owned(),
        },
        accepted_at_unix_ms: 1_000,
    }
}

fn progress(phase: TransferPhase, confirmed_bytes: u64) -> TaskEventKind {
    TaskEventKind::Progress {
        progress: TaskProgress::Transfer(TransferProgress {
            direction: TransferDirection::ToExecutor,
            phase,
            confirmed_bytes,
            total_bytes: Some(100),
            sampled_at_unix_ms: 2_000,
        }),
    }
}

#[test]
fn projects_a_monotonic_task_timeline() {
    let (mut task, accepted) =
        TaskAggregate::accept(accepted(OsFamily::Windows, PathStyle::Windows)).unwrap();
    assert_eq!(accepted.seq, 1);
    assert_eq!(task.record(TaskEventKind::Running, 1_100).unwrap().seq, 2);
    assert_eq!(
        task.record(progress(TransferPhase::Transferring, 40), 2_000)
            .unwrap()
            .seq,
        3
    );
    task.record(progress(TransferPhase::Verifying, 100), 3_000)
        .unwrap();
    task.record(
        TaskEventKind::Succeeded {
            completion: TaskCompletion {
                summary: "Transferred and committed".to_owned(),
                exit_code: None,
            },
        },
        4_000,
    )
    .unwrap();

    let snapshot = task.snapshot();
    assert_eq!(snapshot.state, TaskState::Succeeded);
    assert_eq!(snapshot.latest_event_seq, 5);
    assert_eq!(snapshot.started_at_unix_ms, Some(1_100));
    assert_eq!(snapshot.finished_at_unix_ms, Some(4_000));
    assert!(!snapshot.output.stdout.complete);
    assert_eq!(snapshot.execution_context.os_family, OsFamily::Windows);
    assert_eq!(
        task.record(TaskEventKind::CancelRequested, 5_000),
        Err(TaskRuntimeError::TerminalTask(TaskState::Succeeded))
    );
    task.observe_output(OutputStream::Stdout, 0, 42, true)
        .unwrap();
    assert!(task.snapshot().output.stdout.complete);
    assert_eq!(task.snapshot().output.stdout.available_to, 42);
}

#[test]
fn cancellation_allows_a_real_completion_to_win_the_race() {
    let (mut task, _) = TaskAggregate::accept(accepted(OsFamily::Linux, PathStyle::Posix)).unwrap();
    task.record(TaskEventKind::Running, 2_000).unwrap();
    task.record(TaskEventKind::CancelRequested, 2_100).unwrap();
    task.record(
        TaskEventKind::Succeeded {
            completion: TaskCompletion {
                summary: "Process exited before cancellation".to_owned(),
                exit_code: Some(0),
            },
        },
        2_200,
    )
    .unwrap();
    assert_eq!(task.snapshot().state, TaskState::Succeeded);
}

#[test]
fn rejects_progress_regression_without_mutating_the_snapshot() {
    let (mut task, _) = TaskAggregate::accept(accepted(OsFamily::Macos, PathStyle::Posix)).unwrap();
    task.record(TaskEventKind::Running, 2_000).unwrap();
    task.record(progress(TransferPhase::Transferring, 60), 2_100)
        .unwrap();
    let before = task.snapshot().clone();

    assert_eq!(
        task.record(progress(TransferPhase::Transferring, 50), 2_200),
        Err(TaskRuntimeError::ProgressRegressed)
    );
    assert_eq!(task.snapshot(), &before);
}

#[test]
fn rejects_an_os_path_style_mismatch_at_acceptance() {
    let error = TaskAggregate::accept(accepted(OsFamily::Windows, PathStyle::Posix)).unwrap_err();
    assert_eq!(error, TaskRuntimeError::PathStyleMismatch);
}

#[test]
fn completed_output_ranges_cannot_change() {
    let (mut task, _) = TaskAggregate::accept(accepted(OsFamily::Linux, PathStyle::Posix)).unwrap();
    task.observe_output(OutputStream::Stderr, 10, 40, true)
        .unwrap();
    assert_eq!(
        task.observe_output(OutputStream::Stderr, 10, 41, true),
        Err(TaskRuntimeError::CompletedOutputChanged)
    );
}

#[test]
fn restored_projection_ignores_duplicates_and_reports_gaps() {
    let (mut source, accepted_event) =
        TaskAggregate::accept(accepted(OsFamily::Linux, PathStyle::Posix)).unwrap();
    let running = source.record(TaskEventKind::Running, 2_000).unwrap();
    let mut restored = TaskAggregate::restore(source.snapshot().clone()).unwrap();

    assert_eq!(
        restored.apply_event(&accepted_event),
        Ok(EventApply::Duplicate)
    );
    assert_eq!(restored.apply_event(&running), Ok(EventApply::Duplicate));

    let mut gap = running.clone();
    gap.seq = 4;
    gap.kind = TaskEventKind::CancelRequested;
    assert_eq!(
        restored.apply_event(&gap),
        Err(TaskRuntimeError::EventGap {
            expected: 3,
            actual: 4,
        })
    );
    assert_eq!(restored.snapshot().latest_event_seq, 2);

    gap.seq = 3;
    assert_eq!(restored.apply_event(&gap), Ok(EventApply::Applied));
    assert_eq!(restored.snapshot().state, TaskState::CancelRequested);
}

#[test]
fn restored_projection_rejects_events_for_another_task() {
    let (source, mut event) =
        TaskAggregate::accept(accepted(OsFamily::Linux, PathStyle::Posix)).unwrap();
    let mut restored = TaskAggregate::restore(source.snapshot().clone()).unwrap();
    event.task_ref.task_id = TaskId::from_u128(99);

    assert_eq!(
        restored.apply_event(&event),
        Err(TaskRuntimeError::WrongTask)
    );
    assert_eq!(restored.snapshot().latest_event_seq, 1);
}

#[test]
fn restore_rejects_a_non_terminal_snapshot_with_a_finished_time() {
    let (source, _) =
        TaskAggregate::accept(accepted(OsFamily::Windows, PathStyle::Windows)).unwrap();
    let mut snapshot = source.snapshot().clone();
    snapshot.finished_at_unix_ms = Some(2_000);

    assert_eq!(
        TaskAggregate::restore(snapshot).unwrap_err(),
        TaskRuntimeError::InvalidFinishedTime
    );
}
