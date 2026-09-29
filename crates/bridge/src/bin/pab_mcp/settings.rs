use std::{env, fs, path::Path};

use pab_agent_core::{DataPaths, DataScope};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperatorSettings {
    deployment_id: String,
    control_url: String,
    relay_url: String,
}

#[derive(Debug, Deserialize)]
struct InstalledSettings {
    deployment_id: String,
    control_url: String,
    relay_urls: String,
}

pub fn apply_at_start() -> Result<(), Box<dyn std::error::Error>> {
    let installed_path = env::current_exe()?
        .parent()
        .ok_or("MCP executable has no parent directory")?
        .join("settings.json");
    let user_path = DataPaths::for_scope(DataScope::User)?
        .root()
        .join("operator-server.json");
    let values = load(&installed_path, &user_path)?;
    for (key, value) in values {
        if env::var_os(key).is_none() {
            // This runs before Tokio creates threads, so environment writes are safe.
            unsafe { env::set_var(key, value) };
        }
    }
    Ok(())
}

fn load(
    installed_path: &Path,
    user_path: &Path,
) -> Result<Vec<(&'static str, String)>, Box<dyn std::error::Error>> {
    let mut values = Vec::new();
    if installed_path.exists() {
        let bytes = fs::read(installed_path)?;
        let settings: InstalledSettings = serde_json::from_slice(without_utf8_bom(&bytes))?;
        values = vec![
            ("PAB_DEPLOYMENT_ID", settings.deployment_id),
            ("PAB_CONTROL_URL", settings.control_url),
            ("PAB_RELAY_URLS", settings.relay_urls),
        ];
    }
    if user_path.exists() {
        let bytes = fs::read(user_path)?;
        let settings: OperatorSettings = serde_json::from_slice(without_utf8_bom(&bytes))?;
        values = vec![
            ("PAB_DEPLOYMENT_ID", settings.deployment_id),
            ("PAB_CONTROL_URL", settings.control_url),
            ("PAB_RELAY_URLS", settings.relay_url),
        ];
    }
    Ok(values)
}

fn without_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_settings_override_installed_settings() {
        let root = tempfile::tempdir().unwrap();
        let installed = root.path().join("settings.json");
        let user = root.path().join("operator-server.json");
        fs::write(
            &installed,
            b"\xef\xbb\xbf{\"deployment_id\":\"installed\",\"control_url\":\"wss://installed\",\"relay_urls\":\"https://installed\"}",
        )
        .unwrap();
        fs::write(
            &user,
            r#"{"deploymentId":"user","controlUrl":"wss://user","relayUrl":"https://user"}"#,
        )
        .unwrap();

        assert_eq!(
            load(&installed, &user).unwrap(),
            vec![
                ("PAB_DEPLOYMENT_ID", "user".to_owned()),
                ("PAB_CONTROL_URL", "wss://user".to_owned()),
                ("PAB_RELAY_URLS", "https://user".to_owned()),
            ]
        );
    }
}
