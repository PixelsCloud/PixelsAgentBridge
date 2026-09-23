use pab_agent_core::{DataPaths, DataScope};
use pab_executor::{DeviceStatus, read_local_device_status};
use pab_protocol::ClaimId;

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
    let paths = DataPaths::for_scope(DataScope::Machine)
        .expect("could not find machine data directory");
    pab_logging::init("desktop", paths.root())
        .expect("could not initialize desktop log file");
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![device_status, approve_claim])
        .run(tauri::generate_context!())
        .expect("could not start Pixels Agent Bridge desktop application");
}
