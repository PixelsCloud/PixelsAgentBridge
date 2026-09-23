use std::sync::atomic::Ordering;

use pab_bridge::{
    BridgeLocalStore, BridgeRuntime, LocalTaskRecord, OperationRecord, RuntimeEventKind,
};
use pab_protocol::{OperatorRef, OutputStream};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use super::{ConnectedDevice, OperatorState, parse_code};

const INITIAL_OUTPUT_BYTES: u32 = 32 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryTask {
    id: String,
    device_code: String,
    initiated_by: String,
    program: String,
    args: Vec<String>,
    cwd: Option<String>,
    state: String,
    complete: bool,
    stdout: String,
    stderr: String,
    stdout_offset: u64,
    stderr_offset: u64,
    started_at_unix_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryOperation {
    id: String,
    device_code: String,
    initiated_by: String,
    kind: String,
    direction: String,
    source: String,
    destination: String,
    overwrite: bool,
    state: String,
    offset: u64,
    size: u64,
    started_at_unix_ms: i64,
    finished_at_unix_ms: Option<i64>,
    message: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorBootstrap {
    devices: Vec<ConnectedDevice>,
    tasks: Vec<HistoryTask>,
    operations: Vec<HistoryOperation>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskChanged {
    task_id: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceConnectionChanged {
    device_id: String,
    phase: String,
}

#[tauri::command]
pub async fn operator_bootstrap(
    handle: AppHandle,
    state: State<'_, OperatorState>,
) -> Result<OperatorBootstrap, String> {
    let local = state.local_store().await?;
    let remembered = local
        .remembered_devices()
        .await
        .map_err(|error| error.to_string())?;
    let devices = remembered
        .iter()
        .map(|device| ConnectedDevice {
            device_id: device.device_ref.device_id.to_string(),
            device_code: device.code.to_string(),
            alias: device.alias.clone(),
            os_family: format!("{:?}", device.os_family).to_lowercase(),
            os_reminder: device.os_reminder.clone(),
            connected: false,
        })
        .collect::<Vec<_>>();
    let records = local.tasks().await.map_err(|error| error.to_string())?;
    let mut tasks = Vec::new();
    for record in records
        .into_iter()
        .filter(|record| record.snapshot.is_some())
    {
        let snapshot = record
            .snapshot
            .as_ref()
            .expect("filtered task has a snapshot");
        state
            .tasks
            .lock()
            .await
            .insert(snapshot.task_ref.task_id, snapshot.task_ref);
        let code = remembered
            .iter()
            .find(|device| device.device_ref == record.device_ref)
            .map(|device| device.code.to_string())
            .unwrap_or_else(|| record.device_ref.device_id.to_string());
        tasks.push(history_task(&local, record, code).await?);
    }
    let operations = local
        .operations()
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|record| history_operation(record, &remembered))
        .collect();
    if !state.events_started.swap(true, Ordering::AcqRel) {
        tauri::async_runtime::spawn(async move {
            loop {
                let operator = handle.state::<OperatorState>();
                match operator.runtime().await {
                    Ok(runtime) => {
                        forward_events(handle.clone(), runtime);
                        break;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "operator runtime unavailable");
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    }
                }
            }
        });
    }
    Ok(OperatorBootstrap {
        devices,
        tasks,
        operations,
    })
}

#[tauri::command]
pub async fn operator_operations(
    state: State<'_, OperatorState>,
) -> Result<Vec<HistoryOperation>, String> {
    let local = state.local_store().await?;
    let remembered = local
        .remembered_devices()
        .await
        .map_err(|error| error.to_string())?;
    Ok(local
        .operations()
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|record| history_operation(record, &remembered))
        .collect())
}

fn history_operation(
    record: OperationRecord,
    remembered: &[pab_bridge::RememberedDevice],
) -> HistoryOperation {
    let device_code = remembered
        .iter()
        .find(|device| device.device_ref == record.device_ref)
        .map(|device| device.code.to_string())
        .unwrap_or_else(|| record.device_ref.device_id.to_string());
    HistoryOperation {
        id: record.id,
        device_code,
        initiated_by: record.initiated_by,
        kind: record.kind,
        direction: record.direction,
        source: record.source,
        destination: record.destination,
        overwrite: record.overwrite,
        state: record.state,
        offset: record.offset,
        size: record.size,
        started_at_unix_ms: record.started_at_unix_ms,
        finished_at_unix_ms: record.finished_at_unix_ms,
        message: record.message,
    }
}

async fn history_task(
    local: &BridgeLocalStore,
    record: LocalTaskRecord,
    device_code: String,
) -> Result<HistoryTask, String> {
    let snapshot = record
        .snapshot
        .as_ref()
        .expect("history task has a snapshot");
    let task_ref = snapshot.task_ref;
    let (stdout, _) = local
        .read_output(
            task_ref,
            OutputStream::Stdout,
            record.stdout.retained_from,
            INITIAL_OUTPUT_BYTES,
        )
        .await
        .map_err(|error| error.to_string())?;
    let (stderr, _) = local
        .read_output(
            task_ref,
            OutputStream::Stderr,
            record.stderr.retained_from,
            INITIAL_OUTPUT_BYTES,
        )
        .await
        .map_err(|error| error.to_string())?;
    let stdout_offset = stdout.offset + stdout.bytes.len() as u64;
    let stderr_offset = stderr.offset + stderr.bytes.len() as u64;
    Ok(HistoryTask {
        id: task_ref.task_id.to_string(),
        device_code,
        initiated_by: match snapshot.initiated_by {
            OperatorRef::Account(user_id) => format!("account:{user_id}"),
            OperatorRef::Guest { .. } => "guest".to_owned(),
        },
        program: record
            .command
            .as_ref()
            .map(|command| command.program.clone())
            .unwrap_or_else(|| snapshot.display_summary.clone()),
        args: record
            .command
            .as_ref()
            .map(|command| command.args.clone())
            .unwrap_or_default(),
        cwd: record
            .command
            .as_ref()
            .and_then(|command| command.cwd.clone()),
        state: format!("{:?}", snapshot.state),
        complete: record.is_complete()
            && stdout_offset >= record.stdout.available_to
            && stderr_offset >= record.stderr.available_to,
        stdout: String::from_utf8_lossy(&stdout.bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr.bytes).into_owned(),
        stdout_offset,
        stderr_offset,
        started_at_unix_ms: snapshot.created_at_unix_ms,
    })
}

fn forward_events(handle: AppHandle, runtime: std::sync::Arc<BridgeRuntime>) {
    tauri::async_runtime::spawn(async move {
        let mut events = runtime.subscribe();
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            match event.kind {
                RuntimeEventKind::TaskSnapshot { snapshot, .. } => {
                    let _ = handle.emit(
                        "operator-task-changed",
                        TaskChanged {
                            task_id: snapshot.task_ref.task_id.to_string(),
                        },
                    );
                }
                RuntimeEventKind::TaskEvent { event, .. } => {
                    let _ = handle.emit(
                        "operator-task-changed",
                        TaskChanged {
                            task_id: event.task_ref.task_id.to_string(),
                        },
                    );
                }
                RuntimeEventKind::TaskOutput { chunk, .. } => {
                    let _ = handle.emit(
                        "operator-task-changed",
                        TaskChanged {
                            task_id: chunk.task_ref.task_id.to_string(),
                        },
                    );
                }
                RuntimeEventKind::DeviceConnection {
                    device_ref, phase, ..
                } => {
                    let _ = handle.emit(
                        "operator-device-connection",
                        DeviceConnectionChanged {
                            device_id: device_ref.device_id.to_string(),
                            phase: format!("{phase:?}").to_lowercase(),
                        },
                    );
                }
                _ => {}
            }
        }
    });
}

#[tauri::command]
pub async fn operator_rename_device(
    state: State<'_, OperatorState>,
    code: String,
    alias: String,
) -> Result<String, String> {
    let alias = alias.trim();
    if alias.chars().count() > 64 || alias.chars().any(char::is_control) {
        return Err("device name must be at most 64 printable characters".to_owned());
    }
    state
        .runtime()
        .await?
        .rename_device(parse_code(&code)?, alias)
        .await
        .map_err(|error| error.to_string())?;
    Ok(alias.to_owned())
}
