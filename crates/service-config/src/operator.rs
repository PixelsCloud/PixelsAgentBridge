//! User overrides are deliberately separate from the effective connection settings.
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, path::Path};

/// Restarts inherit the parent's environment, including the applied override.
/// Keep the launch defaults separately so clearing the override also works in
/// development without an installed config.toml. Apply before client::startup;
/// an installed or explicit TOML file still takes precedence over these values.
pub fn launch_defaults(
    mut lookup: impl FnMut(&str) -> Option<OsString>,
) -> Vec<(&'static str, OsString)> {
    let mut values = Vec::new();
    for (name, snapshot) in [
        ("PAB_CONTROL_URL", "PAB_OPERATOR_LAUNCH_CONTROL_URL"),
        ("PAB_RELAY_URLS", "PAB_OPERATOR_LAUNCH_RELAY_URLS"),
    ] {
        if let Some(value) = lookup(snapshot) {
            values.push((name, value));
        } else {
            values.push((snapshot, lookup(name).unwrap_or_default()));
        }
    }
    values
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatorSettings {
    pub control_url: String,
    pub relay_url: String,
}

impl OperatorSettings {
    pub fn normalize(&mut self) {
        self.control_url = self.control_url.trim().to_owned();
        self.relay_url = self.relay_url.trim().to_owned();
    }

    pub fn is_default(&self) -> bool {
        self.control_url.is_empty() && self.relay_url.is_empty()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.is_default() {
            return Ok(());
        }
        if self.control_url.is_empty() || self.relay_url.is_empty() {
            return Err(
                "Fill in both server URLs, or leave both empty to use the default service".into(),
            );
        }
        crate::client::validate_url(&self.control_url, "wss")?;
        crate::client::validate_url(&self.relay_url, "https")?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        Ok(crate::preferences::load(path, |settings: &mut Self| {
            settings.normalize();
            settings.validate()
        })?
        .unwrap_or_default())
    }

    pub fn environment(&self) -> Vec<(&'static str, OsString)> {
        if self.is_default() {
            Vec::new()
        } else {
            vec![
                ("PAB_CONTROL_URL", self.control_url.clone().into()),
                ("PAB_RELAY_URLS", self.relay_url.clone().into()),
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_restores_launch_defaults_instead_of_inherited_user_overrides() {
        let defaults = launch_defaults(|name| match name {
            "PAB_CONTROL_URL" => Some("wss://custom.invalid/control".into()),
            "PAB_OPERATOR_LAUNCH_CONTROL_URL" => Some("wss://default.invalid/control".into()),
            "PAB_OPERATOR_LAUNCH_RELAY_URLS" => Some("https://default-relay.invalid".into()),
            _ => None,
        });
        assert_eq!(
            defaults[0],
            (
                "PAB_CONTROL_URL",
                OsString::from("wss://default.invalid/control")
            )
        );
        assert_eq!(
            defaults[1],
            (
                "PAB_RELAY_URLS",
                OsString::from("https://default-relay.invalid")
            )
        );
        let absent = launch_defaults(|_| None);
        assert!(absent.iter().all(|(_, value)| value.is_empty()));
    }

    #[test]
    fn empty_settings_do_not_replace_installed_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("operator-server.toml");
        let settings = OperatorSettings::load(&path).unwrap();
        assert!(settings.is_default());
        assert!(settings.environment().is_empty());
        crate::preferences::save(&path, &settings).unwrap();
        assert!(
            OperatorSettings::load(&path)
                .unwrap()
                .environment()
                .is_empty()
        );
    }

    #[test]
    fn overrides_are_an_atomic_pair_and_invalid_files_do_not_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("operator-server.toml");
        let mut settings = OperatorSettings {
            control_url: " wss://custom.invalid/control ".into(),
            relay_url: " https://relay.invalid ".into(),
        };
        settings.normalize();
        settings.validate().unwrap();
        crate::preferences::save(&path, &settings).unwrap();
        assert_eq!(OperatorSettings::load(&path).unwrap(), settings);
        assert_eq!(settings.environment().len(), 2);
        settings.relay_url.clear();
        assert!(settings.validate().is_err());
        crate::preferences::save(&path, &settings).unwrap();
        assert!(OperatorSettings::load(&path).is_err());
    }
}
