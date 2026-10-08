use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use pab_bridge::{BridgeRuntime, QueuedTransfer, TransferControl, TransferQueue, TransferRequest};
use pab_protocol::{
    DeviceCode, ExecutionIdentity, ExecutionSelection, FileTransferOptions, RequestId,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::watch;

use super::{OperatorState, parse_code};

pub(super) struct TransferOwner {
    runtime: Arc<BridgeRuntime>,
    queue: TransferQueue,
    code: DeviceCode,
    control: TransferControl,
    cancel: watch::Sender<bool>,
    running: AtomicBool,
    terminal: AtomicBool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferUpdate {
    id: String,
    device_code: String,
    state: String,
    offset: u64,
    size: u64,
    message: Option<String>,
    execution_identity: Option<ExecutionIdentity>,
}

#[derive(Serialize)]
pub struct TransferError {
    message: String,
    request_id: RequestId,
    outcome: &'static str,
}

impl TransferUpdate {
    fn from_record(record: QueuedTransfer) -> Self {
        let operation = record.operation;
        let state = if operation.finished_at_unix_ms.is_none()
            && (record.phase == "unconfirmed"
                || (operation.state != "cancel_requested"
                    && operation.execution_observation.as_deref() == Some("unconfirmed")))
        {
            "unconfirmed".into()
        } else {
            operation.state
        };
        Self {
            id: operation.id,
            device_code: operation
                .device_code
                .map(|c| c.to_string())
                .unwrap_or_default(),
            state,
            offset: operation.offset,
            size: operation.size,
            message: operation.message,
            execution_identity: record.execution_context.and_then(|c| c.identity),
        }
    }
}

async fn observe(owner: &TransferOwner, id: RequestId) -> Result<TransferUpdate, String> {
    let mut record = owner
        .queue
        .get(id, owner.code)
        .await
        .map_err(|e| e.to_string())?;
    // Never ask the remote side before the local stream has stopped. The same
    // owner/runtime survives account switches and no observation resubmits data.
    if !owner.running.load(Ordering::Acquire) && record.operation.finished_at_unix_ms.is_none() {
        let lookup = async {
            let snapshot = owner
                .runtime
                .transfer_status(record.operation.device_ref, id)
                .await
                .map_err(|e| e.to_string())?;
            owner
                .queue
                .reconcile(&record, &snapshot)
                .await
                .map_err(|e| e.to_string())
        };
        let _ = tokio::time::timeout(Duration::from_secs(5), lookup).await;
        record = owner
            .queue
            .get(id, owner.code)
            .await
            .map_err(|e| e.to_string())?;
    }
    owner.terminal.store(
        record.operation.finished_at_unix_ms.is_some(),
        Ordering::Release,
    );
    Ok(TransferUpdate::from_record(record))
}

#[tauri::command]
pub async fn operator_start_transfer(
    app: AppHandle,
    state: State<'_, OperatorState>,
    code: String,
    request_id: RequestId,
    direction: String,
    source: String,
    destination: String,
    overwrite: bool,
    execution: Option<ExecutionSelection>,
) -> Result<TransferUpdate, TransferError> {
    let rejected = |message: String| TransferError {
        message,
        request_id,
        outcome: "not_submitted",
    };
    if !matches!(direction.as_str(), "upload" | "download")
        || source.trim().is_empty()
        || destination.trim().is_empty()
    {
        return Err(rejected(
            "valid direction and source/destination paths are required".into(),
        ));
    }
    let code = parse_code(&code).map_err(rejected)?;
    let runtime = state.runtime().await.map_err(rejected)?;
    let device_ref = runtime
        .resolve_device_code(code)
        .await
        .map_err(|e| rejected(e.to_string()))?;
    let queue = runtime.transfer_queue();
    let spec = TransferRequest {
        request_id,
        device_ref,
        device_code: code,
        direction,
        source,
        destination,
        overwrite,
        options: FileTransferOptions {
            execution: execution.unwrap_or_default(),
            resume_from: None,
        },
    };
    let remembered = queue
        .remembered_device(code)
        .await
        .map_err(|e| rejected(e.to_string()))?;
    spec.validate_paths(remembered.os_family)
        .map_err(|e| rejected(e.into()))?;
    let mut owners = state.transfers.lock().await;
    if let Some(owner) = owners.get(&request_id) {
        if owner.code != code || !Arc::ptr_eq(&owner.runtime, &runtime) {
            return Err(rejected(
                "transfer belongs to another device or account connection".into(),
            ));
        }
    }
    owners.retain(|_, owner| !owner.terminal.load(Ordering::Acquire));
    let existing = queue.get(request_id, code).await.ok();
    if existing.is_none()
        && (owners.len() >= 128
            || owners
                .values()
                .filter(|o| o.running.load(Ordering::Acquire))
                .count()
                >= 8)
    {
        return Err(rejected(
            "transfer capacity reached; inspect existing operations".into(),
        ));
    }
    let (cancel, cancelled) = watch::channel(false);
    let owner = Arc::new(TransferOwner {
        runtime,
        queue,
        code,
        control: TransferControl::default(),
        cancel,
        running: AtomicBool::new(false),
        terminal: AtomicBool::new(false),
    });
    let user = pab_agent_core::account::AccountStore::from_env()
        .and_then(|store| store.read())
        .map_err(|error| rejected(error.to_string()))?
        .user
        .map(|user| pab_protocol::UserAttribution {
            user_id: user.id,
            username: user.username,
        });
    let (record, inserted) = match owner.queue.submit_with_user(&spec, user).await {
        Ok(result) => result,
        Err(error) => {
            // The final read can fail after the acceptance transaction commits.
            // Only proven absence is a pre-submission rejection.
            if matches!(
                owner.queue.get(request_id, code).await,
                Err(pab_bridge::RuntimeStoreError::NotFound)
            ) {
                return Err(rejected(error.to_string()));
            }
            owners.entry(request_id).or_insert(owner);
            return Err(TransferError {
                message: error.to_string(),
                request_id,
                outcome: "unconfirmed",
            });
        }
    };
    if !inserted {
        return Ok(TransferUpdate::from_record(record));
    }
    owner.running.store(true, Ordering::Release);
    owners.insert(request_id, owner.clone());
    tokio::spawn(async move {
        let progress_app = app.clone();
        owner
            .queue
            .run_transfer(
                &spec,
                &owner.control,
                cancelled,
                async { Ok(owner.runtime.clone()) },
                |offset, size| {
                    let _ = progress_app.emit(
                        "operator-transfer",
                        TransferUpdate {
                            id: request_id.to_string(),
                            device_code: code.to_string(),
                            state: if owner.control.cancelled() {
                                "cancel_requested"
                            } else {
                                "running"
                            }
                            .into(),
                            offset,
                            size,
                            message: None,
                            execution_identity: owner
                                .control
                                .accepted()
                                .and_then(|s| s.execution_context)
                                .and_then(|c| c.identity),
                        },
                    );
                },
            )
            .await;
        owner.running.store(false, Ordering::Release);
        match observe(&owner, request_id).await {
            Ok(update) => {
                let _ = app.emit("operator-transfer", update);
            }
            Err(message) => {
                let _ = app.emit(
                    "operator-transfer",
                    TransferUpdate {
                        id: request_id.to_string(),
                        device_code: code.to_string(),
                        state: "unconfirmed".into(),
                        offset: 0,
                        size: 0,
                        message: Some(message),
                        execution_identity: None,
                    },
                );
            }
        }
    });
    Ok(TransferUpdate::from_record(record))
}

#[tauri::command]
pub async fn operator_transfer_result(
    state: State<'_, OperatorState>,
    code: String,
    id: RequestId,
) -> Result<TransferUpdate, String> {
    let code = parse_code(&code)?;
    let owner = state.transfers.lock().await.get(&id).cloned();
    if let Some(owner) = owner {
        if owner.code != code {
            return Err("transfer belongs to another device".into());
        }
        return observe(&owner, id).await;
    }
    // Terminal owners may be evicted. This is a local, read-only lookup subject
    // to the queue's actor/session visibility; never resume a forgotten worker.
    let queue = state.runtime().await?.transfer_queue();
    queue
        .get(id, code)
        .await
        .map(TransferUpdate::from_record)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn operator_cancel_transfer(
    app: AppHandle,
    state: State<'_, OperatorState>,
    code: String,
    id: RequestId,
) -> Result<TransferUpdate, String> {
    let code = parse_code(&code)?;
    let owner = state
        .transfers
        .lock()
        .await
        .get(&id)
        .cloned()
        .ok_or("transfer is no longer active; inspect task history")?;
    if owner.code != code {
        return Err("transfer belongs to another device".into());
    }
    owner
        .queue
        .cancel(id, code)
        .await
        .map_err(|e| e.to_string())?;
    owner.control.cancel();
    let _ = owner.cancel.send(true);
    let update = observe(&owner, id).await?;
    let _ = app.emit("operator-transfer", update.clone());
    Ok(update)
}
