use pab_protocol::{DirectoryEntry, RequestId};
use serde::Serialize;
use tauri::State;

use super::{OperatorState, parse_code};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryPageView {
    request_id: RequestId,
    path: String,
    entries: Vec<DirectoryEntry>,
    next_after: Option<String>,
}

#[tauri::command]
pub async fn operator_list_directory(
    state: State<'_, OperatorState>,
    code: String,
    path: String,
    after: Option<String>,
    execution: Option<pab_protocol::ExecutionSelection>,
) -> Result<DirectoryPageView, String> {
    let runtime = state.runtime().await?;
    let device_ref = runtime
        .resolve_device_code(parse_code(&code)?)
        .await
        .map_err(|error| error.to_string())?;
    let page = runtime
        .list_directory_as(device_ref, &path, after.as_deref(), 8, execution.unwrap_or_default())
        .await
        .map_err(|error| error.to_string())?;
    Ok(DirectoryPageView {
        request_id: page.request_id,
        path: page.path,
        entries: page.entries,
        next_after: page.next_after,
    })
}
