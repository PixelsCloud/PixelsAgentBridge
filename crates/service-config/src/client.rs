//! Installed Executor, Desktop and MCP configuration. No shell is used to read it.
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientConfig {
    pub control_url: String,
    pub relay_urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_ca_cert: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_ca_cert: Option<PathBuf>,
    #[serde(default)]
    pub executor: ProcessConfig,
    #[serde(default)]
    pub operator: ProcessConfig,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProcessConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
}

#[derive(Clone, Copy)]
pub enum Role {
    Executor,
    Operator,
}

impl ClientConfig {
    pub fn validate(&self) -> Result<(), String> {
        validate_url(&self.control_url, "wss")?;
        if self.relay_urls.is_empty() {
            return Err("At least one relay URL is required".into());
        }
        for value in &self.relay_urls {
            validate_url(value, "https")?;
        }
        for process in [&self.executor, &self.operator] {
            for path in [&process.data_dir, &process.log_dir] {
                if path.as_ref().is_some_and(|p| p.as_os_str().is_empty()) {
                    return Err("Configuration paths must not be empty".into());
                }
            }
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let config: Self = crate::load(path).map_err(|e| e.to_string())?;
        config.validate()?;
        Ok(config)
    }

    pub fn environment(&self, path: &Path, role: Role) -> Vec<(&'static str, OsString)> {
        let mut values = vec![
            ("PAB_CONTROL_URL", self.control_url.clone().into()),
            ("PAB_RELAY_URLS", self.relay_urls.join(",").into()),
        ];
        let base = path.parent().unwrap_or(Path::new("."));
        let process = match role {
            Role::Executor => &self.executor,
            Role::Operator => &self.operator,
        };
        for (key, path) in [
            ("PAB_CONTROL_CA_CERT", &self.control_ca_cert),
            ("PAB_RELAY_CA_CERT", &self.relay_ca_cert),
            ("PAB_DATA_DIR", &process.data_dir),
            ("PAB_LOG_DIR", &process.log_dir),
        ] {
            if let Some(path) = path {
                let mut path = path.clone();
                crate::resolve(base, &mut path);
                values.push((key, path.into_os_string()));
            }
        }
        if let Some(level) = &process.log_level {
            values.push(("PAB_LOG_LEVEL", level.into()));
        }
        if let Some(name) = &process.device_name {
            values.push(("PAB_DEVICE_NAME", name.into()));
        }
        values
    }
}

fn validate_url(value: &str, scheme: &str) -> Result<(), String> {
    let url = url::Url::parse(value).map_err(|_| "Invalid server URL")?;
    if url.scheme() != scheme
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || value.chars().any(char::is_whitespace)
        || value.contains(',')
    {
        return Err(
            "Use wss:// control and https:// relay URLs without credentials or whitespace".into(),
        );
    }
    Ok(())
}

pub fn installed_path() -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    {
        Ok("/etc/pixels-agent-bridge/config.toml".into())
    }
    #[cfg(target_os = "macos")]
    {
        Ok("/Library/Application Support/PixelsAgentBridge/config.toml".into())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Ok(std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .ok_or("Executable has no parent directory")?
            .join("config.toml"))
    }
}

pub fn startup(
    role: Role,
    args: impl IntoIterator<Item = OsString>,
) -> Result<(Vec<String>, Vec<(&'static str, OsString)>), String> {
    let args: Vec<_> = args.into_iter().collect();
    let explicit = args.iter().any(|a| a == "--config");
    let (path, rest) = crate::arguments("", args).map_err(|e| e.to_string())?;
    let path = if explicit { path } else { installed_path()? };
    let values = if explicit || path.try_exists().map_err(|e| e.to_string())? {
        ClientConfig::load(&path)?.environment(&path, role)
    } else {
        Vec::new()
    };
    Ok((rest, values))
}

/// Called by installers using the newly staged executable, before replacing binaries.
/// Existing TOML is validated and copied byte-for-byte, including comments.
pub fn configure(args: &[String]) -> Result<(), String> {
    let mut target = None;
    let mut previous = None;
    let mut control = None;
    let mut relay = None;
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        let value = iter.next().ok_or("Configuration option requires a value")?;
        let slot = match flag.as_str() {
            "--config" => &mut target,
            "--previous" => &mut previous,
            "--control-url" => &mut control,
            "--relay-url" => &mut relay,
            _ => return Err("Unknown configuration option".into()),
        };
        if slot.replace(value.as_str()).is_some() {
            return Err("Duplicate configuration option".into());
        }
    }
    let target = Path::new(target.ok_or("configure requires --config")?);
    if target.try_exists().map_err(|e| e.to_string())? {
        ClientConfig::load(target)?;
        return Ok(());
    }
    if let Some(previous) = previous {
        let previous = Path::new(previous);
        let toml = previous.join("config.toml");
        if toml.try_exists().map_err(|e| e.to_string())? {
            ClientConfig::load(&toml)?;
            // The macOS installer stages this file, then publishes it back to the
            // same directory. Preserve comments and relative path spelling.
            return super::preferences::write(
                target,
                &std::fs::read(&toml).map_err(|e| e.to_string())?,
                true,
            );
        }
        if let Some(config) = read_legacy(previous)? {
            config.validate()?;
            let text =
                toml::to_string_pretty(&config).map_err(|_| "Cannot serialize configuration")?;
            return super::preferences::write(target, text.as_bytes(), true);
        }
    }
    let config = ClientConfig {
        control_url: control.unwrap_or("wss://pab.rgaa.vip/control").into(),
        relay_urls: relay
            .unwrap_or("https://pab-relay.rgaa.vip")
            .split(',')
            .map(|s| s.trim().into())
            .collect(),
        control_ca_cert: None,
        relay_ca_cert: None,
        executor: ProcessConfig::default(),
        operator: ProcessConfig::default(),
    };
    config.validate()?;
    let text = toml::to_string_pretty(&config).map_err(|_| "Cannot serialize configuration")?;
    super::preferences::write(target, text.as_bytes(), true)
}

impl ClientConfig {
    fn resolve_paths(&mut self, base: &Path) {
        for path in [
            &mut self.control_ca_cert,
            &mut self.relay_ca_cert,
            &mut self.executor.data_dir,
            &mut self.executor.log_dir,
            &mut self.operator.data_dir,
            &mut self.operator.log_dir,
        ]
        .into_iter()
        .flatten()
        {
            crate::resolve(base, path);
        }
    }
}

fn read_legacy(directory: &Path) -> Result<Option<ClientConfig>, String> {
    let json = directory.join("settings.json");
    let env = directory.join("settings.env");
    let mut values = std::collections::BTreeMap::new();
    if json.try_exists().map_err(|e| e.to_string())? {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Legacy {
            control_url: String,
            relay_urls: String,
        }
        let raw = std::fs::read_to_string(json).map_err(|e| e.to_string())?;
        let old: Legacy = serde_json::from_str(raw.trim_start_matches('\u{feff}'))
            .map_err(|_| "Invalid legacy settings.json; migrate it to TOML manually")?;
        values.insert("PAB_CONTROL_URL".to_owned(), old.control_url);
        values.insert("PAB_RELAY_URLS".to_owned(), old.relay_urls);
    } else if env.try_exists().map_err(|e| e.to_string())? {
        let raw = std::fs::read_to_string(env).map_err(|e| e.to_string())?;
        for line in raw
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let (key, value) = line
                .strip_prefix("export ")
                .unwrap_or(line)
                .split_once('=')
                .ok_or("Cannot migrate legacy shell configuration; write config.toml manually")?;
            if values
                .insert(key.to_owned(), shell_literal(value)?)
                .is_some()
            {
                return Err(
                    "Duplicate legacy configuration key; write config.toml manually".into(),
                );
            }
        }
    } else {
        return Ok(None);
    }
    let mut config = ClientConfig {
        control_url: values
            .remove("PAB_CONTROL_URL")
            .ok_or("Legacy control URL missing")?,
        relay_urls: values
            .remove("PAB_RELAY_URLS")
            .ok_or("Legacy relay URL missing")?
            .split(',')
            .map(|s| s.trim().into())
            .collect(),
        control_ca_cert: values.remove("PAB_CONTROL_CA_CERT").map(Into::into),
        relay_ca_cert: values.remove("PAB_RELAY_CA_CERT").map(Into::into),
        executor: ProcessConfig {
            // Old Unix service launchers always forced the machine data root.
            // A data override in settings.env therefore belonged to the MCP user.
            data_dir: None,
            log_dir: values.remove("PAB_LOG_DIR").map(Into::into),
            log_level: values.remove("PAB_LOG_LEVEL"),
            device_name: values.remove("PAB_DEVICE_NAME"),
        },
        operator: ProcessConfig {
            data_dir: values.remove("PAB_DATA_DIR").map(Into::into),
            ..Default::default()
        },
    };
    if !values.is_empty() {
        return Err("Unrecognized legacy configuration; migrate it to TOML manually".into());
    }
    config.operator.log_dir = config.executor.log_dir.clone();
    config.operator.log_level = config.executor.log_level.clone();
    // Old shell launchers resolved relative paths against their caller's cwd;
    // guessing that directory could switch device identity or user storage.
    for path in [
        &config.control_ca_cert,
        &config.relay_ca_cert,
        &config.operator.data_dir,
        &config.executor.log_dir,
    ]
    .into_iter()
    .flatten()
    {
        if path.is_relative() {
            return Err("Legacy relative paths require manual TOML migration".into());
        }
    }
    config.resolve_paths(directory);
    Ok(Some(config))
}

// Only literal assignments emitted by the old installers are accepted. Never eval/source.
fn shell_literal(text: &str) -> Result<String, String> {
    let error =
        || "Legacy configuration contains shell syntax; migrate it to TOML manually".to_owned();
    let mut output = String::new();
    let mut quote = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), c) => output.push(c),
            (None, '\'' | '"') => quote = Some(c),
            (_, '\\') => output.push(chars.next().ok_or_else(error)?),
            (_, '$' | '`') | (None, ';' | '|' | '&' | '<' | '>' | '(' | ')') => return Err(error()),
            (None, c) if c.is_whitespace() => return Err(error()),
            (_, c) => output.push(c),
        }
    }
    if quote.is_some() {
        return Err(error());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migrate_preserve_and_reject_executable_configuration() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config.toml");
        let legacy = root.path().join("settings.env");
        std::fs::write(&legacy, "export PAB_CONTROL_URL=wss://self.test/control\nexport PAB_RELAY_URLS=https://relay.test\n").unwrap();
        let args = vec![
            "--config".into(),
            target.to_str().unwrap().into(),
            "--previous".into(),
            root.path().to_str().unwrap().into(),
        ];
        configure(&args).unwrap();
        assert_eq!(
            ClientConfig::load(&target).unwrap().control_url,
            "wss://self.test/control"
        );
        let text = format!(
            "# user comment\n{}",
            std::fs::read_to_string(&target).unwrap()
        );
        std::fs::write(&target, &text).unwrap();
        configure(&args).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), text);
        std::fs::remove_file(&target).unwrap();
        std::fs::write(
            &legacy,
            "export PAB_CONTROL_URL=$(touch /tmp/should-not-exist)",
        )
        .unwrap();
        assert!(configure(&args).is_err());
        assert!(!target.exists());
    }
    #[test]
    fn explicit_missing_invalid_and_role_separation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        assert!(
            startup(
                Role::Operator,
                [OsString::from("--config"), path.clone().into_os_string()]
            )
            .is_err()
        );
        std::fs::write(&path, "control_url='wss://host.test'\nrelay_urls=['https://relay.test']\n[executor]\ndata_dir='data'\n").unwrap();
        let config = ClientConfig::load(&path).unwrap();
        assert!(
            config
                .environment(&path, Role::Executor)
                .iter()
                .any(|(k, v)| *k == "PAB_DATA_DIR" && *v == root.path().join("data"))
        );
        assert!(
            !config
                .environment(&path, Role::Operator)
                .iter()
                .any(|(k, _)| *k == "PAB_DATA_DIR")
        );
        std::fs::write(&path, "control_url='secret invalid url'\nrelay_urls=[]\n").unwrap();
        assert!(!ClientConfig::load(&path).unwrap_err().contains("secret"));
    }

    #[test]
    fn legacy_user_storage_never_replaces_the_machine_identity_directory() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("user data");
        let legacy = root.path().join("settings.env");
        let header =
            "export PAB_CONTROL_URL=wss://host.test\nexport PAB_RELAY_URLS=https://relay.test\n";
        std::fs::write(
            &legacy,
            format!(
                "{header}export PAB_DATA_DIR='{}'\nexport PAB_LOG_LEVEL=warn\n",
                data.display()
            ),
        )
        .unwrap();
        let config = read_legacy(root.path()).unwrap().unwrap();
        assert_eq!(config.operator.data_dir, Some(data));
        assert!(config.executor.data_dir.is_none());
        assert_eq!(config.operator.log_level.as_deref(), Some("warn"));
        std::fs::write(&legacy, format!("{header}export PAB_DATA_DIR=relative\n")).unwrap();
        assert!(read_legacy(root.path()).is_err());
    }
    #[test]
    fn literal_shell_parser_handles_quotes_without_expansion() {
        assert_eq!(
            shell_literal("wss://host/a\\?x=1\\&y=2").unwrap(),
            "wss://host/a?x=1&y=2"
        );
        assert_eq!(shell_literal("'a b'").unwrap(), "a b");
        for text in ["$(id)", "`id`", "url;id", "\"$HOME\"", "'broken"] {
            assert!(shell_literal(text).is_err());
        }
    }
    #[test]
    fn windows_bom_json_migrates_and_staged_upgrade_keeps_comments() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config.toml");
        std::fs::write(root.path().join("settings.json"),
            "\u{feff}{\"control_url\":\"wss://windows.test\",\"relay_urls\":\"https://one.test,https://two.test\"}").unwrap();
        configure(&[
            "--config".into(),
            target.display().to_string(),
            "--previous".into(),
            root.path().display().to_string(),
        ])
        .unwrap();
        assert_eq!(ClientConfig::load(&target).unwrap().relay_urls.len(), 2);
        let text = format!(
            "# keep comments\n{}",
            std::fs::read_to_string(&target).unwrap()
        );
        std::fs::write(&target, &text).unwrap();
        let staged = root.path().join("stage/config.toml");
        configure(&[
            "--config".into(),
            staged.display().to_string(),
            "--previous".into(),
            root.path().display().to_string(),
        ])
        .unwrap();
        assert_eq!(std::fs::read_to_string(staged).unwrap(), text);
    }
}
