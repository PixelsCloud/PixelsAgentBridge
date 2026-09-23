use pab_agent_core::{DataPaths, DataScope};
use pab_executor::{DeviceStatus, read_local_device_status};
use pab_protocol::ClaimId;

mod operator;

#[tauri::command]
fn device_status() -> Result<DeviceStatus, String> {
    read_local_device_status().map_err(|error| error.to_string())
}

#[tauri::command]
async fn approve_claim(claim_id: String) -> Result<(), String> {
    let claim_id = claim_id
        .parse::<ClaimId>()
        .map_err(|_| "申请 ID 格式不正确".to_owned())?;
    pab_executor::approve_claim(claim_id)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "device claim approval failed");
            error.to_string()
        })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let paths = DataPaths::for_scope(DataScope::User).expect("could not find user data directory");
    pab_logging::init("desktop", paths.root()).expect("could not initialize desktop log file");
    tauri::Builder::default()
        .manage(operator::OperatorState::new())
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
