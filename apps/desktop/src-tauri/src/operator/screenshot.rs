use base64::{Engine, engine::general_purpose::STANDARD};
use pab_protocol::RequestId;
use serde::Serialize;
use tauri::State;

use super::{OperatorState, parse_code};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotView {
    request_id: RequestId,
    width: u32,
    height: u32,
    size: u64,
    sha256: String,
    data_url: String,
}

#[tauri::command]
pub async fn operator_capture_screenshot(
    state: State<'_, OperatorState>,
    code: String,
) -> Result<ScreenshotView, String> {
    let runtime = state.runtime().await?;
    let device_ref = runtime
        .resolve_device_code(parse_code(&code)?)
        .await
        .map_err(|error| error.to_string())?;
    let image = runtime
        .capture_screenshot(device_ref, None)
        .await
        .map_err(|error| error.to_string())?;
    Ok(screenshot_view(image))
}

#[tauri::command]
pub async fn operator_preview_desktop(
    state: State<'_, OperatorState>,
    code: String,
) -> Result<ScreenshotView, String> {
    let runtime = state.runtime().await?;
    let device_ref = runtime
        .resolve_device_code(parse_code(&code)?)
        .await
        .map_err(|error| error.to_string())?;
    let image = runtime
        .preview_screenshot(device_ref)
        .await
        .map_err(|error| error.to_string())?;
    Ok(screenshot_view(image))
}

fn screenshot_view(image: pab_bridge::Screenshot) -> ScreenshotView {
    ScreenshotView {
        request_id: image.meta.request_id,
        width: image.meta.width,
        height: image.meta.height,
        size: image.meta.size,
        sha256: image.meta.sha256,
        data_url: format!("data:image/png;base64,{}", STANDARD.encode(image.bytes)),
    }
}

#[tauri::command]
pub async fn operator_screenshot_from_history(
    state: State<'_, OperatorState>,
    id: String,
) -> Result<Option<String>, String> {
    let local = state.local_store().await?;
    let bytes = local
        .screenshot_bytes(&id)
        .await
        .map_err(|error| error.to_string())?;
    Ok(bytes.map(|bytes| format!("data:image/png;base64,{}", STANDARD.encode(bytes))))
}
