use std::{env, ffi::OsString, path::PathBuf, time::Duration};

use iroh_base::RelayUrl;
use pab_agent_core::{DataPathError, DataPaths, DataScope};
use pab_protocol::{DeploymentId, TenantId, UserId};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeConfig {
    pub deployment_id: DeploymentId,
    pub tenant_id: TenantId,
    pub user_id: UserId,
    pub control_url: String,
    pub relay_urls: Vec<RelayUrl>,
    pub endpoint_secret_file: PathBuf,
    pub control_ca_cert: Option<PathBuf>,
    pub relay_ca_cert: Option<PathBuf>,
    pub operation_timeout: Duration,
}

impl BridgeConfig {
    pub fn from_env() -> Result<Self, BridgeConfigError> {
        Self::from_lookup(|name| env::var_os(name))
    }

    fn from_lookup(
        mut lookup: impl FnMut(&str) -> Option<OsString>,
    ) -> Result<Self, BridgeConfigError> {
        let endpoint_secret_file = match lookup("PAB_ENDPOINT_SECRET_FILE") {
            Some(path) => PathBuf::from(path),
            None => {
                DataPaths::for_scope_with(DataScope::User, &mut lookup)?.bridge_endpoint_secret()
            }
        };
        let config = Self {
            deployment_id: required_text(&mut lookup, "PAB_DEPLOYMENT_ID")?
                .parse()
                .map_err(|error| invalid("PAB_DEPLOYMENT_ID", error))?,
            tenant_id: required_text(&mut lookup, "PAB_TENANT_ID")?
                .parse()
                .map_err(|error| invalid("PAB_TENANT_ID", error))?,
            user_id: required_text(&mut lookup, "PAB_USER_ID")?
                .parse()
                .map_err(|error| invalid("PAB_USER_ID", error))?,
            control_url: required_text(&mut lookup, "PAB_CONTROL_URL")?,
            relay_urls: relay_urls(required_text(&mut lookup, "PAB_RELAY_URLS")?)?,
            endpoint_secret_file,
            control_ca_cert: lookup("PAB_CONTROL_CA_CERT").map(PathBuf::from),
            relay_ca_cert: lookup("PAB_RELAY_CA_CERT").map(PathBuf::from),
            operation_timeout: Duration::from_secs(10),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), BridgeConfigError> {
        if !self.control_url.starts_with("wss://") {
            return Err(BridgeConfigError::ControlUrlMustUseTls);
        }
        if self.relay_urls.is_empty() {
            return Err(BridgeConfigError::NoRelayUrls);
        }
        if let Some(url) = self
            .relay_urls
            .iter()
            .find(|relay_url| relay_url.scheme() != "https")
        {
            return Err(BridgeConfigError::RelayTlsRequired(url.to_string()));
        }
        if self.operation_timeout.is_zero() {
            return Err(BridgeConfigError::ZeroOperationTimeout);
        }
        Ok(())
    }
}

fn relay_urls(value: String) -> Result<Vec<RelayUrl>, BridgeConfigError> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .parse()
                .map_err(|error| invalid("PAB_RELAY_URLS", error))
        })
        .collect()
}

fn required_text(
    lookup: &mut impl FnMut(&str) -> Option<OsString>,
    name: &'static str,
) -> Result<String, BridgeConfigError> {
    lookup(name)
        .ok_or(BridgeConfigError::Missing(name))?
        .into_string()
        .map_err(|_| BridgeConfigError::NotUnicode(name))
}

fn invalid(name: &'static str, error: impl std::fmt::Display) -> BridgeConfigError {
    BridgeConfigError::Invalid {
        name,
        message: error.to_string(),
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BridgeConfigError {
    #[error(transparent)]
    DataPath(#[from] DataPathError),
    #[error("{0} must be set")]
    Missing(&'static str),
    #[error("{0} must contain valid Unicode")]
    NotUnicode(&'static str),
    #[error("{name} is invalid: {message}")]
    Invalid { name: &'static str, message: String },
    #[error("PAB_CONTROL_URL must use wss://")]
    ControlUrlMustUseTls,
    #[error("PAB_RELAY_URLS must contain at least one Relay URL")]
    NoRelayUrls,
    #[error("Relay URL must use HTTPS: {0}")]
    RelayTlsRequired(String),
    #[error("the Bridge operation timeout must be greater than zero")]
    ZeroOperationTimeout,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn default_endpoint_key_uses_persistent_user_data() {
        let values = HashMap::from([
            ("PAB_DATA_DIR", OsString::from("persistent-user-data")),
            (
                "PAB_DEPLOYMENT_ID",
                OsString::from("00000000-0000-0000-0000-000000000001"),
            ),
            (
                "PAB_TENANT_ID",
                OsString::from("00000000-0000-0000-0000-000000000002"),
            ),
            (
                "PAB_USER_ID",
                OsString::from("00000000-0000-0000-0000-000000000003"),
            ),
            (
                "PAB_CONTROL_URL",
                OsString::from("wss://pab.example/control"),
            ),
            ("PAB_RELAY_URLS", OsString::from("https://relay.example")),
        ]);
        let config = BridgeConfig::from_lookup(|name| values.get(name).cloned()).unwrap();
        assert_eq!(
            config.endpoint_secret_file,
            PathBuf::from("persistent-user-data/bridge-endpoint.key")
        );
    }
}
