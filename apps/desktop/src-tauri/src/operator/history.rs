use std::sync::atomic::Ordering;

use pab_bridge::{
    BridgeLocalStore, BridgeRuntime, LocalTaskRecord, OperationRecord, RuntimeEventKind,
};
use pab_protocol::{DeviceRef, OperatorRef, OutputStream, RequestId};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use super::{ConnectedDevice, OperatorState, parse_code};

const INITIAL_OUTPUT_BYTES: u32 = 32 * 1024;
const HISTORY_PAGE_SIZE: usize = 40;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryTask {
    execution_identity: Option<pab_protocol::ExecutionIdentity>,
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
    execution_identity: Option<pab_protocol::ExecutionIdentity>,
    ui: Option<pab_protocol::UiOperationSummary>,
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
    execution_observation: Option<String>,
    phase: Option<String>,
    mutation: Option<HistoryMutation>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryMutation {
    phase: String,
    total_entries: u32,
    processed_entries: u32,
    published_entries: u32,
    deleted_entries: u32,
    partial: bool,
    source_removed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorBootstrap {
    devices: Vec<ConnectedDevice>,
    #[serde(flatten)]
    history: HistoryPage,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    tasks: Vec<HistoryTask>,
    operations: Vec<HistoryOperation>,
    total_count: u64,
    task_before: Option<String>,
    operation_before_started_at_unix_ms: Option<i64>,
    operation_before_id: Option<String>,
    has_more_tasks: bool,
    has_more_operations: bool,
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
    let history =
        load_history_page(&local, &remembered, &state, None, None, true, true, None).await?;
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
    Ok(OperatorBootstrap { devices, history })
}

#[tauri::command]
pub async fn operator_history_page(
    state: State<'_, OperatorState>,
    task_before: Option<String>,
    operation_before_started_at_unix_ms: Option<i64>,
    operation_before_id: Option<String>,
    load_tasks: bool,
    load_operations: bool,
) -> Result<HistoryPage, String> {
    let task_before = task_before
        .as_deref()
        .map(str::parse::<RequestId>)
        .transpose()
        .map_err(|_| "invalid task history cursor".to_owned())?;
    let operation_before = match (
        operation_before_started_at_unix_ms,
        operation_before_id.as_deref(),
    ) {
        (Some(started_at), Some(id)) => Some((started_at, id)),
        (None, None) => None,
        _ => return Err("incomplete operation history cursor".to_owned()),
    };
    let local = state.local_store().await?;
    let remembered = local
        .remembered_devices()
        .await
        .map_err(|error| error.to_string())?;
    load_history_page(
        &local,
        &remembered,
        &state,
        task_before,
        operation_before,
        load_tasks,
        load_operations,
        None,
    )
    .await
}

#[tauri::command]
pub async fn operator_device_history_page(
    state: State<'_, OperatorState>,
    code: String,
    task_before: Option<String>,
    operation_before_started_at_unix_ms: Option<i64>,
    operation_before_id: Option<String>,
    load_tasks: bool,
    load_operations: bool,
) -> Result<HistoryPage, String> {
    let code = parse_code(&code)?;
    let local = state.local_store().await?;
    let remembered = local
        .remembered_devices()
        .await
        .map_err(|error| error.to_string())?;
    let device_ref = remembered
        .iter()
        .find(|device| device.code == code)
        .map(|device| device.device_ref)
        .ok_or_else(|| "saved device was not found".to_owned())?;
    let task_before = task_before
        .as_deref()
        .map(str::parse::<RequestId>)
        .transpose()
        .map_err(|_| "invalid task history cursor".to_owned())?;
    let operation_before = match (
        operation_before_started_at_unix_ms,
        operation_before_id.as_deref(),
    ) {
        (Some(started_at), Some(id)) => Some((started_at, id)),
        (None, None) => None,
        _ => return Err("incomplete operation history cursor".to_owned()),
    };
    load_history_page(
        &local,
        &remembered,
        &state,
        task_before,
        operation_before,
        load_tasks,
        load_operations,
        Some(device_ref),
    )
    .await
}

async fn load_history_page(
    local: &BridgeLocalStore,
    remembered: &[pab_bridge::RememberedDevice],
    state: &OperatorState,
    task_before: Option<RequestId>,
    operation_before: Option<(i64, &str)>,
    load_tasks: bool,
    load_operations: bool,
    device_ref: Option<DeviceRef>,
) -> Result<HistoryPage, String> {
    let mut tasks = Vec::new();
    let mut task_cursor = None;
    let mut has_more_tasks = false;
    if load_tasks {
        let mut records = match device_ref {
            Some(device_ref) => {
                local
                    .tasks_page_for_device(device_ref, task_before, HISTORY_PAGE_SIZE as u32 + 1)
                    .await
            }
            None => {
                local
                    .tasks_page(task_before, HISTORY_PAGE_SIZE as u32 + 1)
                    .await
            }
        }
        .map_err(|error| error.to_string())?;
        has_more_tasks = records.len() > HISTORY_PAGE_SIZE;
        records.truncate(HISTORY_PAGE_SIZE);
        task_cursor = records.last().map(|record| record.request_id.to_string());
        for record in records {
            let snapshot = record
                .snapshot
                .as_ref()
                .expect("history query requires a task snapshot");
            let endpoint_key = match snapshot.initiated_by {
                OperatorRef::Account { endpoint_key, .. } => endpoint_key,
                OperatorRef::Guest { guest_endpoint_key } => guest_endpoint_key,
            };
            if let Some(runtime) = state
                .runtimes
                .lock()
                .await
                .endpoints
                .get(&endpoint_key)
                .cloned()
            {
                state
                    .tasks
                    .lock()
                    .await
                    .entry(snapshot.task_ref.task_id)
                    .or_insert((snapshot.task_ref, runtime));
            }
            let code = remembered
                .iter()
                .find(|device| device.device_ref == record.device_ref)
                .map(|device| device.code.to_string())
                .unwrap_or_else(|| record.device_ref.device_id.to_string());
            tasks.push(history_task(local, record, code).await?);
        }
    }

    let mut operations = Vec::new();
    let mut operation_cursor = None;
    let mut has_more_operations = false;
    if load_operations {
        let mut records = match device_ref {
            Some(device_ref) => {
                local
                    .operations_page_for_device(
                        device_ref,
                        operation_before,
                        HISTORY_PAGE_SIZE as u32 + 1,
                    )
                    .await
            }
            None => {
                local
                    .operations_page(operation_before, HISTORY_PAGE_SIZE as u32 + 1)
                    .await
            }
        }
        .map_err(|error| error.to_string())?;
        has_more_operations = records.len() > HISTORY_PAGE_SIZE;
        records.truncate(HISTORY_PAGE_SIZE);
        operation_cursor = records
            .last()
            .map(|record| (record.started_at_unix_ms, record.id.clone()));
        operations = records
            .into_iter()
            .map(|record| history_operation(record, remembered))
            .collect();
    }

    Ok(HistoryPage {
        tasks,
        operations,
        total_count: local
            .history_count(device_ref)
            .await
            .map_err(|error| error.to_string())?,
        task_before: task_cursor,
        operation_before_started_at_unix_ms: operation_cursor.as_ref().map(|value| value.0),
        operation_before_id: operation_cursor.map(|value| value.1),
        has_more_tasks,
        has_more_operations,
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
        .operations_refresh()
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
        .or_else(|| record.device_code.map(|code| code.to_string()))
        .unwrap_or_else(|| record.device_ref.device_id.to_string());
    HistoryOperation {
        execution_identity: record.execution_identity,
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
        execution_observation: record.execution_observation,
        phase: record.transfer_phase,
        ui: record.ui,
        mutation: record.filesystem_mutation.map(|m| HistoryMutation {
            phase: m.phase,
            total_entries: m.total_entries,
            processed_entries: m.processed_entries,
            published_entries: m.published_entries,
            deleted_entries: m.deleted_entries,
            partial: m.partial,
            source_removed: m.source_removed,
        }),
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
    let (stdout, stdout_range) = local
        .read_output(
            task_ref,
            OutputStream::Stdout,
            record.stdout.retained_from,
            INITIAL_OUTPUT_BYTES,
        )
        .await
        .map_err(|error| error.to_string())?;
    let (stderr, stderr_range) = local
        .read_output(
            task_ref,
            OutputStream::Stderr,
            record.stderr.retained_from,
            INITIAL_OUTPUT_BYTES,
        )
        .await
        .map_err(|error| error.to_string())?;
    let stdout_text = pab_bridge::output_text::decode_output(&stdout.bytes, Default::default(),
        stdout_range.complete && stdout.offset + stdout.bytes.len() as u64 >= stdout_range.available_to);
    let stderr_text = pab_bridge::output_text::decode_output(&stderr.bytes, Default::default(),
        stderr_range.complete && stderr.offset + stderr.bytes.len() as u64 >= stderr_range.available_to);
    let stdout_offset = stdout.offset + stdout_text.consumed as u64;
    let stderr_offset = stderr.offset + stderr_text.consumed as u64;
    Ok(HistoryTask {
        execution_identity: snapshot.execution_context.identity.clone(),
        id: task_ref.task_id.to_string(),
        device_code,
        initiated_by: match snapshot.initiated_by {
            OperatorRef::Account {
                user_id,
                endpoint_key,
            } => {
                let bytes = endpoint_key.as_bytes();
                format!(
                    "account:{user_id}@{:02x}{:02x}{:02x}{:02x}",
                    bytes[0], bytes[1], bytes[2], bytes[3]
                )
            }
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
        stdout: stdout_text.text,
        stderr: stderr_text.text,
        stdout_offset,
        stderr_offset,
        started_at_unix_ms: snapshot.created_at_unix_ms,
    })
}

pub(super) fn forward_events(handle: AppHandle, runtime: std::sync::Arc<BridgeRuntime>) {
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
