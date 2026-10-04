use pab_agent_core::{DataPaths, DataScope, ensure_data_dir};
use pab_protocol::DeploymentId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettings {
    deployment_id: String,
    control_url: String,
    relay_url: String,
}

impl ServerSettings {
    fn validate(&self) -> Result<(), String> {
        self.deployment_id
            .parse::<DeploymentId>()
            .map_err(|_| "Invalid deployment ID".to_owned())?;
        validate_url(&self.control_url, "wss://")?;
        validate_url(&self.relay_url, "https://")?;
        Ok(())
    }
}

fn validate_url(value: &str, scheme: &str) -> Result<(), String> {
    let parsed = url::Url::parse(value).map_err(|_| "Invalid server URL".to_owned())?;
    if parsed.scheme() != scheme.trim_end_matches("://")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || value.chars().any(char::is_whitespace)
    {
        return Err("Invalid server URL".to_owned());
    }
    Ok(())
}

fn settings_path() -> Result<PathBuf, String> {
    let paths = DataPaths::for_scope(DataScope::User).map_err(|error| error.to_string())?;
    Ok(paths.root().join("operator-server.json"))
}

fn read_saved() -> Result<Option<ServerSettings>, String> {
    let path = settings_path()?;
    #[cfg(target_os = "macos")]
    let path = if path.exists() {
        path
    } else {
        PathBuf::from("/Library/Application Support/PixelsAgentBridge/operator-server.json")
    };
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read(path).map_err(|error| error.to_string())?;
    let settings: ServerSettings =
        serde_json::from_slice(&content).map_err(|error| error.to_string())?;
    settings.validate()?;
    Ok(Some(settings))
}

pub fn apply_saved_at_start() -> Result<(), String> {
    let Some(settings) = read_saved()? else {
        return Ok(());
    };
    // Called before Tauri or Tokio starts threads. The operator uses this
    // user-scoped override; the machine service retains its own configuration.
    unsafe {
        std::env::set_var("PAB_DEPLOYMENT_ID", settings.deployment_id);
        std::env::set_var("PAB_CONTROL_URL", settings.control_url);
        std::env::set_var("PAB_RELAY_URLS", settings.relay_url);
    }
    Ok(())
}

#[tauri::command]
pub fn get_operator_server_settings() -> Result<ServerSettings, String> {
    Ok(ServerSettings {
        deployment_id: std::env::var("PAB_DEPLOYMENT_ID").unwrap_or_default(),
        control_url: std::env::var("PAB_CONTROL_URL").unwrap_or_default(),
        relay_url: std::env::var("PAB_RELAY_URLS").unwrap_or_default(),
    })
}

#[tauri::command]
pub fn save_operator_server_settings(settings: ServerSettings) -> Result<(), String> {
    settings.validate()?;
    let path = settings_path()?;
    let parent = path.parent().ok_or("Settings path has no parent")?;
    ensure_data_dir(parent).map_err(|error| error.to_string())?;
    let content = serde_json::to_vec_pretty(&settings).map_err(|error| error.to_string())?;
    std::fs::write(&path, content).map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn restart_desktop(app: tauri::AppHandle) {
    app.restart();
}
