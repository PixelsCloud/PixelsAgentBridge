use pab_protocol::{DesktopInputEvent, RequestId, WindowEntry};
use serde::Serialize;
use tauri::State;

use super::{OperatorState, parse_code};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowListView {
    request_id: RequestId,
    entries: Vec<WindowEntry>,
}

#[tauri::command]
pub async fn operator_list_windows(
    state: State<'_, OperatorState>,
    code: String,
) -> Result<WindowListView, String> {
    let runtime = state.runtime().await?;
    let device_ref = runtime
        .resolve_device_code(parse_code(&code)?)
        .await
        .map_err(|error| error.to_string())?;
    let list = runtime
        .list_windows(device_ref)
        .await
        .map_err(|error| error.to_string())?;
    Ok(WindowListView {
        request_id: list.request_id,
        entries: list.entries,
    })
}

#[tauri::command]
pub async fn operator_desktop_input(
    state: State<'_, OperatorState>,
    code: String,
    event: DesktopInputEvent,
) -> Result<(), String> {
    let runtime = state.runtime().await?;
    let device_ref = runtime
        .resolve_device_code(parse_code(&code)?)
        .await
        .map_err(|error| error.to_string())?;
    runtime
        .desktop_input(device_ref, event)
        .await
        .map_err(|error| error.to_string())
}
