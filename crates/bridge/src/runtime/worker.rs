use std::sync::Arc;

use pab_protocol::{DeviceTaskResponse, TargetContext, TaskSnapshot};
use tokio::sync::oneshot;

use super::{DeviceSession, LocalTaskRecord, RuntimeError, RuntimeEventKind, RuntimeInner};

pub(super) async fn run_record(
    runtime: Arc<RuntimeInner>,
    mut record: LocalTaskRecord,
    accepted: &mut Option<oneshot::Sender<Result<TaskSnapshot, String>>>,
) -> Result<(), RuntimeError> {
    let device = runtime.device(record.device_ref).await;
    if record.snapshot.is_none() {
        let target = read_environment(&device).await?;
        runtime.publish(RuntimeEventKind::CommandPending {
            device_ref: record.device_ref,
            request_id: record.request_id,
            target: target.clone(),
            display_summary: record
                .command
                .as_ref()
                .ok_or(RuntimeError::MissingPendingCommand)?
                .display_summary
                .clone(),
        });
        let command = record
            .command
            .clone()
            .ok_or(RuntimeError::MissingPendingCommand)?;
        if target.execution.os_family != command.expected_environment.os_family
            || target.execution.environment_revision
                != command.expected_environment.environment_revision
        {
            return Err(RuntimeError::EnvironmentChanged);
        }
        let snapshot = loop {
            let connection = device.connection().await?;
            match connection
                .submit_command(record.request_id, command.clone())
                .await
            {
                Ok(snapshot) => break snapshot,
                Err(error) if error.is_recoverable_connection() => {
                    runtime.publish_task_retry(Some(target.clone()), record.request_id, &error);
                    device.recover(&connection, &error).await?;
                }
                Err(error) => return Err(error.into()),
            }
        };
        record = runtime.store.bind_snapshot(&snapshot).await?;
        runtime.publish_snapshot(&snapshot);
        if let Some(sender) = accepted.take() {
            let _ = sender.send(Ok(snapshot));
        }
    } else if let Some(sender) = accepted.take() {
        let _ = sender.send(Ok(record.snapshot.clone().expect("snapshot checked")));
    }
    follow_record(runtime, device, record).await
}

async fn read_environment(device: &DeviceSession) -> Result<TargetContext, RuntimeError> {
    loop {
        let connection = device.connection().await?;
        match connection.get_environment().await {
            Ok(target) => return Ok(target),
            Err(error) if error.is_recoverable_connection() => {
                device.recover(&connection, &error).await?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

async fn follow_record(
    runtime: Arc<RuntimeInner>,
    device: Arc<DeviceSession>,
    mut record: LocalTaskRecord,
) -> Result<(), RuntimeError> {
    if record.is_complete() {
        return Ok(());
    }
    let task_ref = record
        .snapshot
        .as_ref()
        .ok_or(RuntimeError::MissingTaskSnapshot)?
        .task_ref;
    loop {
        let connection = device.connection().await?;
        let remote_snapshot = match connection.get_task(task_ref).await {
            Ok(snapshot) => snapshot,
            Err(error) if error.is_recoverable_connection() => {
                runtime.publish_task_retry(
                    record.snapshot.as_ref().map(TaskSnapshot::target_context),
                    record.request_id,
                    &error,
                );
                device.recover(&connection, &error).await?;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let (reconciled, gaps) = runtime.store.reconcile_snapshot(&remote_snapshot).await?;
        record = reconciled;
        let target = remote_snapshot.target_context();
        for gap in gaps {
            runtime.publish(RuntimeEventKind::TaskOutputGap {
                target: target.clone(),
                task_ref,
                stream: gap.stream,
                missing_from: gap.missing_from,
                missing_to: gap.missing_to,
            });
        }
        runtime.publish_snapshot(&remote_snapshot);
        if record.is_complete() {
            return Ok(());
        }
        let mut subscription = match connection
            .subscribe_task(
                task_ref,
                record.last_event_seq,
                record.stdout.available_to,
                record.stderr.available_to,
            )
            .await
        {
            Ok(subscription) => subscription,
            Err(error) if error.is_recoverable_connection() => {
                runtime.publish_task_retry(
                    record.snapshot.as_ref().map(TaskSnapshot::target_context),
                    record.request_id,
                    &error,
                );
                device.recover(&connection, &error).await?;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        loop {
            match subscription.next().await {
                Ok(DeviceTaskResponse::Event { event }) => {
                    if runtime.store.record_event(&event).await? {
                        let target = record
                            .snapshot
                            .as_ref()
                            .ok_or(RuntimeError::MissingTaskSnapshot)?
                            .target_context();
                        runtime.publish(RuntimeEventKind::TaskEvent { target, event });
                    }
                    record = runtime.store.get_by_request(record.request_id).await?;
                }
                Ok(DeviceTaskResponse::Output { chunk, range }) => {
                    if runtime.store.append_output(&chunk, &range).await? {
                        let target = record
                            .snapshot
                            .as_ref()
                            .ok_or(RuntimeError::MissingTaskSnapshot)?
                            .target_context();
                        runtime.publish(RuntimeEventKind::TaskOutput {
                            target,
                            chunk,
                            range,
                        });
                    }
                    record = runtime.store.get_by_request(record.request_id).await?;
                }
                Ok(DeviceTaskResponse::CaughtUp { snapshot }) => {
                    record = runtime.store.update_snapshot(&snapshot).await?;
                    runtime.publish_snapshot(&snapshot);
                    if record.is_complete() {
                        return Ok(());
                    }
                }
                Ok(DeviceTaskResponse::OutputChanged { .. }) => {}
                Ok(response) => {
                    return Err(RuntimeError::UnexpectedResponse(format!("{response:?}")));
                }
                Err(error) if error.is_recoverable_connection() => {
                    runtime.publish_task_retry(
                        record.snapshot.as_ref().map(TaskSnapshot::target_context),
                        record.request_id,
                        &error,
                    );
                    device.recover(&connection, &error).await?;
                    break;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}
