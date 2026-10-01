use super::{RuntimeError, RuntimeEvent, RuntimeEventKind, RuntimeInner};
use crate::desktop_presence::{DeviceReport, OperationReport, RuntimeReport, TaskReport, now_ms};
use pab_protocol::{DeviceRef, TaskEventKind};
use std::sync::Arc;

pub struct RuntimePresenceSource {
    pub(super) inner: Arc<RuntimeInner>,
}

impl RuntimePresenceSource {
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<RuntimeEvent> {
        self.inner.events.subscribe()
    }

    /// Reads cached/live state only; never authenticates a device or starts an operation.
    pub async fn snapshot(&self) -> Result<RuntimeReport, RuntimeError> {
        let mut report = self
            .inner
            .presence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let remembered = self.inner.store.remembered_devices().await?;
        let codes = self.inner.device_codes.lock().await.clone();
        let sessions = self.inner.devices.lock().await.clone();
        for device in &mut report.devices {
            if let Some(code) = codes.get(&device.device_ref) {
                device.device_code = Some(code.to_string());
            }
            if let Some(saved) = remembered
                .iter()
                .find(|saved| saved.device_ref == device.device_ref)
            {
                device.device_code = Some(saved.code.to_string());
                if !saved.alias.is_empty() {
                    device.alias = Some(saved.alias.clone());
                }
            }
            device.connection_path = match sessions.get(&device.device_ref) {
                Some(session) => session.selected_path().await.map(|path| {
                    match path {
                        crate::ConnectionPath::Direct => "p2p",
                        crate::ConnectionPath::Relay => "relay",
                        crate::ConnectionPath::Unknown => "unknown",
                    }
                    .to_owned()
                }),
                None => None,
            };
            if device.phase == "connected" && device.connection_path.is_none() {
                device.phase = "disconnected".to_owned();
            }
        }
        report.operations = self
            .inner
            .store
            .operations_for_session(&self.inner.session_id)
            .await?
            .into_iter()
            .map(|op| OperationReport {
                id: op.id,
                device_ref: op.device_ref,
                device_code: op.device_code.map(|code| code.to_string()),
                // Report only known file paths; never include text contents or command arguments.
                source: if matches!(
                    op.kind.as_str(),
                    "file_transfer"
                        | "file_stat"
                        | "file_read"
                        | "file_write"
                        | "file_patch"
                        | "file_search"
                        | "file_hash"
                        | "mkdir"
                        | "file_copy"
                        | "file_move"
                        | "file_delete"
                        | "archive_create"
                        | "archive_extract"
                        | "containers"
                        | "container"
                        | "container_logs"
                        | "container_control"
                        | "git_status"
                        | "git_diff"
                        | "git_log"
                        | "git_commit"
                        | "git_checkout"
                        | "git_fetch"
                        | "git_pull"
                        | "git_push"
                ) {
                    op.source
                } else {
                    String::new()
                },
                destination: if op.kind == "file_transfer" {
                    op.destination
                } else {
                    String::new()
                },
                kind: op.kind,
                direction: op.direction,
                state: if op.state == "running"
                    && op.execution_observation.as_deref() == Some("unconfirmed")
                {
                    "unconfirmed".to_owned()
                } else {
                    op.state
                },
                completed_bytes: op.offset,
                total_bytes: op.size,
                started_at_unix_ms: op.started_at_unix_ms,
                finished_at_unix_ms: op.finished_at_unix_ms,
            })
            .collect();
        report.sampled_at_unix_ms = now_ms();
        Ok(report)
    }
}

pub(super) fn device_entry(report: &mut RuntimeReport, device_ref: DeviceRef) -> &mut DeviceReport {
    if let Some(index) = report
        .devices
        .iter()
        .position(|device| device.device_ref == device_ref)
    {
        return &mut report.devices[index];
    }
    report.devices.push(DeviceReport {
        device_ref,
        device_code: None,
        name: None,
        alias: None,
        phase: "resolved".to_owned(),
        connection_path: None,
        retry_in_ms: None,
        last_error: None,
        changed_at_unix_ms: now_ms(),
        environment: None,
    });
    report.devices.last_mut().unwrap()
}

pub(super) fn apply_event(report: &mut RuntimeReport, event: &RuntimeEvent) {
    match &event.kind {
        RuntimeEventKind::DeviceConnection {
            device_ref,
            phase,
            retry_in_ms,
            message,
        } => {
            let device = device_entry(report, *device_ref);
            device.phase = serde_json::to_value(phase)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned();
            device.retry_in_ms = *retry_in_ms;
            device.last_error = message.clone();
            device.changed_at_unix_ms = event.occurred_at_unix_ms;
        }
        RuntimeEventKind::CommandPending {
            device_ref,
            request_id,
            target,
            ..
        } => {
            device_entry(report, *device_ref).environment = Some(target.execution.clone());
            if !report
                .tasks
                .iter()
                .any(|task| task.request_id == *request_id)
            {
                report.tasks.push(TaskReport {
                    request_id: *request_id,
                    task_id: None,
                    device_ref: *device_ref,
                    capability: "process.exec".to_owned(),
                    state: "pending".to_owned(),
                    stage: None,
                    progress: None,
                    created_at_unix_ms: event.occurred_at_unix_ms,
                    started_at_unix_ms: None,
                    finished_at_unix_ms: None,
                    exit_code: None,
                    error_code: None,
                    output: Default::default(),
                });
            }
        }
        RuntimeEventKind::TaskSnapshot { target, snapshot } => {
            device_entry(report, target.device_ref).environment = Some(target.execution.clone());
            let task = TaskReport {
                request_id: snapshot.request_id,
                task_id: Some(snapshot.task_ref.task_id.to_string()),
                device_ref: target.device_ref,
                capability: snapshot.capability.name.clone(),
                state: serde_json::to_value(snapshot.state)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned(),
                stage: snapshot.stage.clone(),
                progress: snapshot.progress.clone(),
                created_at_unix_ms: snapshot.created_at_unix_ms,
                started_at_unix_ms: snapshot.started_at_unix_ms,
                finished_at_unix_ms: snapshot.finished_at_unix_ms,
                exit_code: snapshot
                    .completion
                    .as_ref()
                    .and_then(|v| v.exit_code)
                    .or_else(|| snapshot.error.as_ref().and_then(|v| v.exit_code)),
                error_code: snapshot.error.as_ref().map(|v| v.code.clone()),
                output: snapshot.output.clone(),
            };
            if let Some(previous) = report
                .tasks
                .iter_mut()
                .find(|value| value.request_id == task.request_id)
            {
                *previous = task;
            } else {
                report.tasks.push(task);
            }
        }
        RuntimeEventKind::TaskEvent {
            event: task_event, ..
        } => {
            if let Some(task) = report.tasks.iter_mut().find(|task| {
                task.task_id.as_deref() == Some(&task_event.task_ref.task_id.to_string())
            }) {
                match &task_event.kind {
                    TaskEventKind::Accepted => task.state = "accepted".to_owned(),
                    TaskEventKind::Running => {
                        task.state = "running".to_owned();
                        task.started_at_unix_ms = Some(task_event.occurred_at_unix_ms);
                    }
                    TaskEventKind::StageChanged { stage } => task.stage = stage.clone(),
                    TaskEventKind::Progress { progress } => task.progress = Some(progress.clone()),
                    TaskEventKind::CancelRequested => task.state = "cancel_requested".to_owned(),
                    TaskEventKind::Succeeded { completion } => {
                        task.state = "succeeded".to_owned();
                        task.exit_code = completion.exit_code;
                        task.finished_at_unix_ms = Some(task_event.occurred_at_unix_ms);
                    }
                    TaskEventKind::Failed { error } => {
                        task.state = "failed".to_owned();
                        task.error_code = Some(error.code.clone());
                        task.exit_code = error.exit_code;
                        task.finished_at_unix_ms = Some(task_event.occurred_at_unix_ms);
                    }
                    TaskEventKind::Cancelled { .. } => {
                        task.state = "cancelled".to_owned();
                        task.finished_at_unix_ms = Some(task_event.occurred_at_unix_ms);
                    }
                    TaskEventKind::Interrupted { .. } => {
                        task.state = "interrupted".to_owned();
                        task.finished_at_unix_ms = Some(task_event.occurred_at_unix_ms);
                    }
                }
            }
        }
        RuntimeEventKind::TaskOutput { chunk, range, .. } => {
            if let Some(task) = report
                .tasks
                .iter_mut()
                .find(|task| task.task_id.as_deref() == Some(&chunk.task_ref.task_id.to_string()))
            {
                match chunk.stream {
                    pab_protocol::OutputStream::Stdout => task.output.stdout = range.clone(),
                    pab_protocol::OutputStream::Stderr => task.output.stderr = range.clone(),
                }
            }
        }
        RuntimeEventKind::TaskStopped { request_id, .. } => {
            if let Some(task) = report
                .tasks
                .iter_mut()
                .find(|task| task.request_id == *request_id && task.finished_at_unix_ms.is_none())
            {
                task.state = "interrupted".to_owned();
                task.finished_at_unix_ms = Some(event.occurred_at_unix_ms);
                task.error_code = Some("runtime_stopped".to_owned());
            }
        }
        _ => {}
    }
    if matches!(
        event.kind,
        RuntimeEventKind::TaskOutput { .. }
            | RuntimeEventKind::TaskOutputGap { .. }
            | RuntimeEventKind::DeviceConnection { .. }
    ) {
        return;
    }
    report.tasks.sort_by_key(|task| {
        std::cmp::Reverse(task.finished_at_unix_ms.unwrap_or(task.created_at_unix_ms))
    });
    let mut completed = 0;
    report.tasks.retain(|task| {
        if task.finished_at_unix_ms.is_some() {
            completed += 1;
            completed <= 100
        } else {
            true
        }
    });
}
