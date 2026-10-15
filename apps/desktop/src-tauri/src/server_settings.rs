use pab_agent_core::{DataPaths, DataScope, ensure_data_dir};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerSettings {
    control_url: String,
    relay_url: String,
}

impl ServerSettings {
    fn validate(&self) -> Result<(), String> {
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
    Ok(paths.root().join("operator-server.toml"))
}

fn read_saved() -> Result<Option<ServerSettings>, String> {
    let path = settings_path()?;
    pab_service_config::preferences::load(&path, |settings: &mut ServerSettings| {
        settings.validate()
    })
}

pub fn apply_saved_at_start() -> Result<(), String> {
    let explicit = std::env::args_os().any(|a| a == "--config");
    let (_, values) = pab_service_config::client::startup(
        pab_service_config::client::Role::Operator,
        std::env::args_os().skip(1),
    )?;
    // Called before Tauri or Tokio creates application threads.
    for (key, value) in values {
        unsafe {
            std::env::set_var(key, value);
        }
    }
    if explicit {
        return Ok(());
    }
    let Some(settings) = read_saved()? else {
        return Ok(());
    };
    // Called before Tauri or Tokio starts threads. The operator uses this
    // user-scoped override; the machine service retains its own configuration.
    unsafe {
        std::env::set_var("PAB_CONTROL_URL", settings.control_url);
        std::env::set_var("PAB_RELAY_URLS", settings.relay_url);
    }
    Ok(())
}

#[tauri::command]
pub fn get_operator_server_settings() -> Result<ServerSettings, String> {
    Ok(ServerSettings {
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
    pab_service_config::preferences::save(&path, &settings)
}

#[tauri::command]
pub fn restart_desktop(app: tauri::AppHandle) {
    app.restart();
}
