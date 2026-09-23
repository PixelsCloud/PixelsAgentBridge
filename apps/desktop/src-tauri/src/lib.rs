use pab_agent_core::{DataPaths, DataScope};
use pab_executor::{DeviceStatus, read_local_device_status};
use pab_protocol::ClaimId;
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tokio::sync::RwLock;

mod operator;

#[derive(Clone, Default)]
struct LocalStatus(Arc<RwLock<Option<DeviceStatus>>>);

#[tauri::command]
async fn device_status(state: tauri::State<'_, LocalStatus>) -> Result<DeviceStatus, String> {
    if let Some(status) = state.0.read().await.clone() {
        return Ok(status);
    }
    read_local_device_status().map_err(|error| error.to_string())
}

#[tauri::command]
async fn approve_claim(claim_id: String) -> Result<(), String> {
    let claim_id = claim_id
        .parse::<ClaimId>()
        .map_err(|_| "申请 ID 格式不正确".to_owned())?;
    let token_exists = pab_executor::local_ipc::user_token_path()
        .ok()
        .is_some_and(|path| path.is_file());
    if token_exists {
        pab_executor::local_ipc::approve_local_claim(claim_id)
            .await
            .map_err(|error| error.to_string())
    } else {
        pab_executor::approve_claim(claim_id)
            .await
            .map_err(|error| {
                tracing::warn!(%error, "device claim approval failed");
                error.to_string()
            })
    }
}

async fn watch_local_service(handle: tauri::AppHandle, status: LocalStatus) {
    loop {
        match pab_executor::local_ipc::connect_local().await {
            Ok(mut socket) => loop {
                match tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    pab_executor::local_ipc::next_status(&mut socket),
                )
                .await
                {
                    Ok(Ok(next)) => {
                        *status.0.write().await = Some(next.clone());
                        let _ = handle.emit("local-device-status", next);
                    }
                    Ok(Err(error)) => {
                        tracing::debug!(%error, "local WebSocket disconnected");
                        break;
                    }
                    Err(_) => break,
                }
            },
            Err(error) => tracing::debug!(%error, "local WebSocket unavailable"),
        }
        *status.0.write().await = None;
        let _ = handle.emit("local-device-offline", ());
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let paths = DataPaths::for_scope(DataScope::User).expect("could not find user data directory");
    pab_logging::init("desktop", paths.root()).expect("could not initialize desktop log file");
    tauri::Builder::default()
        .manage(LocalStatus::default())
        .manage(operator::OperatorState::new())
        .setup(|app| {
            let handle = app.handle().clone();
            let status = app.state::<LocalStatus>().inner().clone();
            tauri::async_runtime::spawn(watch_local_service(handle, status));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            device_status,
            approve_claim,
            operator::operator_connect,
            operator::operator_run_command,
            operator::operator_task,
            operator::operator_claim,
        ])
        .run(tauri::generate_context!())
        .expect("could not start Pixels Agent Bridge desktop application");
}
