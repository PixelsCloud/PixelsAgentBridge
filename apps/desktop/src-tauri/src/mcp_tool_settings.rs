use pab_bridge::mcp_tool_settings::{McpToolSettings, ToolGroup};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolGroupInfo {
    group: ToolGroup,
    tool_count: usize,
    required: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSettingsView {
    settings: McpToolSettings,
    groups: Vec<ToolGroupInfo>,
    load_error: Option<String>,
}

#[tauri::command]
pub async fn get_mcp_tool_settings() -> Result<ToolSettingsView, String> {
    tauri::async_runtime::spawn_blocking(|| {
        // The editor shows defaults with the read error so users can explicitly
        // repair corrupt settings. MCP startup itself remains strict.
        let (settings, load_error) = match McpToolSettings::load(&McpToolSettings::path()?) {
            Ok(settings) => (settings, None),
            Err(error) => (McpToolSettings::default(), Some(error)),
        };
        Ok(ToolSettingsView {
            settings,
            load_error,
            groups: ToolGroup::ALL
                .into_iter()
                .map(|group| ToolGroupInfo {
                    group,
                    tool_count: group.tools().len(),
                    required: group == ToolGroup::Core,
                })
                .collect(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn save_mcp_tool_settings(settings: McpToolSettings) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || settings.save(&McpToolSettings::path()?))
        .await
        .map_err(|e| e.to_string())?
}
