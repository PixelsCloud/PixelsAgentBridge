//! One TOML contract, also used to validate settings edited by the Android UI.

use serde::{Deserialize, Serialize};

pub const MAX_CONFIG_BYTES: usize = 64 * 1024;

/// Global user intent is independent of server identities and connection lifetimes.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControlIntent {
    pub schema_version: u16,
    pub paused: bool,
    pub connection_requested: bool,
}

impl ControlIntent {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != 1 {
            return Err("unsupported_intent_version");
        }
        Ok(())
    }

    pub fn parse(text: &str) -> Result<Self, &'static str> {
        if text.len() > 4096 {
            return Err("intent_too_large");
        }
        let intent: Self = toml::from_str(text).map_err(|_| "invalid_intent")?;
        intent.validate()?;
        Ok(intent)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidConfig {
    pub schema_version: u16,
    pub control_url: String,
    pub device_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted_ca_pem: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relay_urls: Vec<String>,
    #[cfg(debug_assertions)]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub relay_only_for_testing: bool,
}

impl AndroidConfig {
    pub fn defaults() -> Self {
        Self {
            schema_version: 1,
            control_url: pab_service_config::client::DEFAULT_CONTROL_URL.into(),
            device_name: "Pixels Android".into(),
            trusted_ca_pem: None,
            relay_urls: vec![pab_service_config::client::DEFAULT_RELAY_URL.into()],
            #[cfg(debug_assertions)]
            relay_only_for_testing: false,
        }
    }

    pub fn parse(text: &str) -> Result<Self, &'static str> {
        if text.len() > MAX_CONFIG_BYTES {
            return Err("config_too_large");
        }
        let config: Self = toml::from_str(text).map_err(|_| "invalid_toml")?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != 1 {
            return Err("unsupported_config_version");
        }
        let url = url::Url::parse(&self.control_url).map_err(|_| "invalid_control_url")?;
        if url.scheme() != "wss"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.query().is_some()
            || self.control_url.len() > 2048
            || self.control_url.trim() != self.control_url
            || self.control_url.chars().any(char::is_control)
        {
            return Err("invalid_control_url");
        }
        if self.device_name.trim().is_empty()
            || self.device_name.len() > 128
            || self.device_name.chars().any(char::is_control)
        {
            return Err("invalid_device_name");
        }
        if let Some(pem) = &self.trusted_ca_pem {
            if pem.len() > 48 * 1024 {
                return Err("invalid_ca");
            }
            pab_agent_core::tls_connector(Some(pem.as_bytes())).map_err(|_| "invalid_ca")?;
        }
        if self.relay_urls.len() > 8
            || self.relay_urls.iter().any(|text| {
                url::Url::parse(text).map_or(true, |url| {
                    url.scheme() != "https"
                        || url.host_str().is_none()
                        || !url.username().is_empty()
                        || url.password().is_some()
                        || url.query().is_some()
                        || url.fragment().is_some()
                        || text.len() > 2048
                })
            })
        {
            return Err("invalid_relay_url");
        }
        Ok(())
    }

    /// Bind credentials to the exact control endpoint, including its path.
    pub fn credential_scope(&self) -> String {
        url::Url::parse(&self.control_url)
            .expect("validated config")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_pause_is_explicit_and_unknown_intent_never_silently_resumes() {
        let intent = ControlIntent {
            schema_version: 1,
            paused: true,
            connection_requested: false,
        };
        let source = toml::to_string(&intent).unwrap();
        assert_eq!(ControlIntent::parse(&source).unwrap(), intent);
        for text in [
            "schema_version = 2\npaused = true\nconnection_requested = false",
            "schema_version = 1\nconnection_requested = false",
            "schema_version = 1\npaused = false\nconnection_requested = false\nrun = 'anything'",
        ] {
            assert!(ControlIntent::parse(text).is_err());
        }
    }

    #[test]
    fn rejects_plaintext_embedded_credentials_and_unsupported_configuration() {
        for url in [
            "ws://host/control",
            "wss://user:pass@host/control",
            "wss://host/#token",
            "wss://host/?token=x",
        ] {
            let config = AndroidConfig {
                schema_version: 1,
                control_url: url.into(),
                device_name: "手机".into(),
                trusted_ca_pem: None,
                relay_urls: Vec::new(),
                #[cfg(debug_assertions)]
                relay_only_for_testing: false,
            };
            assert_eq!(config.validate(), Err("invalid_control_url"));
        }
        assert!(
            AndroidConfig::parse(
                "schema_version = 2\ncontrol_url = 'wss://host/control'\ndevice_name = '手机'"
            )
            .is_err()
        );
        assert!(AndroidConfig::parse("schema_version = 1\ncontrol_url = 'wss://host/control'\ndevice_name = '手机'\nsecret = 'ignored'").is_err());
    }

    #[test]
    fn toml_round_trip_preserves_quoted_and_multilingual_names() {
        let config = AndroidConfig {
            schema_version: 1,
            control_url: "wss://example.com/control".into(),
            device_name: "我的\"手机\\Pixels".into(),
            trusted_ca_pem: None,
            relay_urls: Vec::new(),
            #[cfg(debug_assertions)]
            relay_only_for_testing: false,
        };
        let encoded = toml::to_string_pretty(&config).unwrap();
        assert_eq!(
            AndroidConfig::parse(&encoded).unwrap().device_name,
            config.device_name
        );
        assert_eq!(config.credential_scope(), "wss://example.com/control");
    }
}
