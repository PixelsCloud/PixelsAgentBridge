use pab_agent_core::{DataPaths, DataScope, ensure_data_dir};
use pab_service_config::operator::OperatorSettings;
use std::{path::PathBuf, sync::OnceLock};

pub type ServerSettings = OperatorSettings;
static DEFAULTS: OnceLock<ServerSettings> = OnceLock::new();

fn settings_path() -> Result<PathBuf, String> {
    let paths = DataPaths::for_scope(DataScope::User).map_err(|error| error.to_string())?;
    Ok(paths.root().join("operator-server.toml"))
}

fn effective_environment() -> ServerSettings {
    ServerSettings {
        control_url: std::env::var("PAB_CONTROL_URL").unwrap_or_default(),
        relay_url: std::env::var("PAB_RELAY_URLS").unwrap_or_default(),
    }
}

pub fn apply_saved_at_start() -> Result<(), String> {
    for (key, value) in pab_service_config::operator::launch_defaults(|key| std::env::var_os(key)) {
        // Startup is single-threaded; preserve defaults across app.restart().
        unsafe {
            std::env::set_var(key, value);
        }
    }
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
    let _ = DEFAULTS.set(effective_environment());
    if !explicit {
        let settings = ServerSettings::load(&settings_path()?)?;
        for (key, value) in settings.environment() {
            unsafe {
                std::env::set_var(key, value);
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn get_operator_server_settings() -> Result<ServerSettings, String> {
    ServerSettings::load(&settings_path()?)
}

fn account_console_url(control_url: &str) -> Result<String, String> {
    pab_agent_core::account::account_origin(control_url)
        .map(|origin| format!("{origin}/"))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn open_account_console(window: tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("only the main window can open the account console".into());
    }
    // Use the running account service, including --config and embedded defaults.
    // Saved settings may be blank, or awaiting restart; never fall back to an
    // official website when a custom service is selected. No token enters the URL.
    let target = account_console_url(&effective_environment().control_url)?;
    webbrowser::open(&target)
        .map_err(|_| "could not open the account console in the default browser".into())
}

#[tauri::command]
pub async fn validate_operator_server(
    kind: pab_agent_core::server_probe::ServerKind,
    url: String,
) -> Result<pab_agent_core::server_probe::ProbeSuccess, pab_agent_core::server_probe::ProbeError> {
    use pab_agent_core::server_probe::{ProbeError, ServerKind};
    let variable = match kind {
        ServerKind::Control => "PAB_CONTROL_CA_CERT",
        ServerKind::Relay => "PAB_RELAY_CA_CERT",
    };
    let ca = std::env::var_os(variable)
        .map(std::fs::read)
        .transpose()
        .map_err(|_| ProbeError::Tls)?;
    let ca = ca.as_deref().or_else(|| {
        let pem = pab_service_config::client::DEFAULT_RELAY_CA_PEM.as_bytes();
        (matches!(kind, ServerKind::Relay) && !pem.is_empty()).then_some(pem)
    });
    pab_agent_core::server_probe::probe(kind, &url, ca).await
}

#[tauri::command]
pub async fn save_operator_server_settings(
    mut settings: ServerSettings,
    state: tauri::State<'_, crate::operator::OperatorState>,
    reporting: tauri::State<'_, crate::mcp_reporting::McpReportingState>,
    github: tauri::State<'_, crate::github_login::GithubLoginState>,
) -> Result<(), String> {
    settings.normalize();
    settings.validate()?;
    if std::env::args_os().any(|a| a == "--config") {
        return Err("This process uses --config; edit that file to change servers".into());
    }
    let next = if settings.is_default() {
        DEFAULTS.get().cloned().unwrap_or_default()
    } else {
        settings.clone()
    };
    let current = effective_environment();
    let _guard = state.account_changes.lock().await;
    let path = settings_path()?;
    ensure_data_dir(path.parent().ok_or("Settings path has no parent")?)
        .map_err(|error| error.to_string())?;
    // Validate and publish the override before changing account state. Runtime
    // connections still use the old configuration until the app is restarted.
    // If cleanup fails, restore the previous override and report the error.
    let previous = ServerSettings::load(&path)?;
    pab_service_config::preferences::save(&path, &settings)?;
    if current.control_url != next.control_url {
        crate::github_login::github_cancel(github).await?;
        let result = clear_server_accounts(&current.control_url, &next.control_url);
        match result {
            Ok(revision) => {
                if let Some(revision) = revision {
                    reporting.account_changed(revision);
                    pab_agent_core::account::notify_account_change(revision);
                }
            }
            Err(error) => {
                pab_service_config::preferences::save(&path, &previous)?;
                return Err(error);
            }
        }
    }
    Ok(())
}

fn clear_server_accounts(current: &str, next: &str) -> Result<Option<u64>, String> {
    let paths = DataPaths::for_scope(DataScope::User).map_err(|e| e.to_string())?;
    let mut revision = None;
    for server in [current, next].into_iter().filter(|s| !s.is_empty()) {
        let store = pab_agent_core::account::AccountStore::new(paths.root(), server)
            .map_err(|e| e.to_string())?;
        let snapshot = store.clear_local_session().map_err(|e| e.to_string())?;
        if server == current {
            revision = Some(snapshot.revision);
        }
    }
    Ok(revision)
}

#[tauri::command]
pub fn restart_desktop(app: tauri::AppHandle) {
    app.restart();
}

#[cfg(test)]
mod console_tests {
    use super::account_console_url;

    #[test]
    fn console_follows_the_current_account_service_origin() {
        for (control, expected) in [
            ("wss://default.invalid/control", "https://default.invalid/"),
            (
                "wss://customer.invalid:8443/control",
                "https://customer.invalid:8443/",
            ),
            (
                "wss://[::1]:9443/nested/control?session=private",
                "https://[::1]:9443/",
            ),
        ] {
            assert_eq!(account_console_url(control).unwrap(), expected);
        }
    }

    #[test]
    fn missing_or_unsafe_services_never_fall_back_to_another_site() {
        for control in [
            "",
            "javascript:alert(1)",
            "http://customer.invalid/control",
            "wss://user:secret@customer.invalid/control",
            "wss://customer.invalid/control#fragment",
        ] {
            assert!(account_console_url(control).is_err());
        }
    }
}
