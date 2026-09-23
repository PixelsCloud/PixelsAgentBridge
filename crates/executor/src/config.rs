use std::{env, ffi::OsString, path::PathBuf, time::Duration};

use iroh_base::RelayUrl;
use pab_agent_core::{DataPathError, DataPaths, DataScope};
use pab_protocol::{DeploymentId, DeviceId, TenantId};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutorConfig {
    pub deployment_id: DeploymentId,
    pub tenant_id: TenantId,
    pub device_id: DeviceId,
    pub control_url: String,
    pub relay_urls: Vec<RelayUrl>,
    pub endpoint_secret_file: PathBuf,
    pub device_credential_file: PathBuf,
    pub task_database_file: PathBuf,
    pub control_ca_cert: Option<PathBuf>,
    pub relay_ca_cert: Option<PathBuf>,
    pub operation_timeout: Duration,
}

impl ExecutorConfig {
    pub fn from_env() -> Result<Self, ExecutorConfigError> {
        Self::from_lookup(|name| env::var_os(name))
    }

    pub(crate) fn from_lookup(
        mut lookup: impl FnMut(&str) -> Option<OsString>,
    ) -> Result<Self, ExecutorConfigError> {
        let endpoint_secret_path = lookup("PAB_ENDPOINT_SECRET_FILE").map(PathBuf::from);
        let credential_path = lookup("PAB_DEVICE_CREDENTIAL_FILE").map(PathBuf::from);
        let database_path = lookup("PAB_TASK_DATABASE").map(PathBuf::from);
        let data_paths = if endpoint_secret_path.is_none()
            || credential_path.is_none()
            || database_path.is_none()
        {
            Some(DataPaths::for_scope_with(DataScope::Machine, &mut lookup)?)
        } else {
            None
        };
        let config = Self {
            deployment_id: required_text(&mut lookup, "PAB_DEPLOYMENT_ID")?
                .parse()
                .map_err(|error| invalid("PAB_DEPLOYMENT_ID", error))?,
            tenant_id: required_text(&mut lookup, "PAB_TENANT_ID")?
                .parse()
                .map_err(|error| invalid("PAB_TENANT_ID", error))?,
            device_id: required_text(&mut lookup, "PAB_DEVICE_ID")?
                .parse()
                .map_err(|error| invalid("PAB_DEVICE_ID", error))?,
            control_url: required_text(&mut lookup, "PAB_CONTROL_URL")?,
            relay_urls: relay_urls(required_text(&mut lookup, "PAB_RELAY_URLS")?)?,
            endpoint_secret_file: endpoint_secret_path.unwrap_or_else(|| {
                data_paths
                    .as_ref()
                    .expect("default paths requested")
                    .executor_endpoint_secret()
            }),
            device_credential_file: credential_path.unwrap_or_else(|| {
                data_paths
                    .as_ref()
                    .expect("default paths requested")
                    .executor_credential()
            }),
            task_database_file: database_path.unwrap_or_else(|| {
                data_paths
                    .as_ref()
                    .expect("default paths requested")
                    .executor_database()
            }),
            control_ca_cert: lookup("PAB_CONTROL_CA_CERT").map(PathBuf::from),
            relay_ca_cert: lookup("PAB_RELAY_CA_CERT").map(PathBuf::from),
            operation_timeout: Duration::from_secs(10),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ExecutorConfigError> {
        if !self.control_url.starts_with("wss://") {
            return Err(ExecutorConfigError::ControlUrlMustUseTls);
        }
        if self.relay_urls.is_empty() {
            return Err(ExecutorConfigError::NoRelayUrls);
        }
        if let Some(url) = self
            .relay_urls
            .iter()
            .find(|relay_url| relay_url.scheme() != "https")
        {
            return Err(ExecutorConfigError::RelayTlsRequired(url.to_string()));
        }
        if self.operation_timeout.is_zero() {
            return Err(ExecutorConfigError::ZeroOperationTimeout);
        }
        Ok(())
    }
}

fn relay_urls(value: String) -> Result<Vec<RelayUrl>, ExecutorConfigError> {
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
) -> Result<String, ExecutorConfigError> {
    lookup(name)
        .ok_or(ExecutorConfigError::Missing(name))?
        .into_string()
        .map_err(|_| ExecutorConfigError::NotUnicode(name))
}

fn invalid(name: &'static str, error: impl std::fmt::Display) -> ExecutorConfigError {
    ExecutorConfigError::Invalid {
        name,
        message: error.to_string(),
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExecutorConfigError {
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
    #[error("the control operation timeout must be greater than zero")]
    ZeroOperationTimeout,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn valid_values() -> HashMap<String, OsString> {
        HashMap::from([
            ("PAB_DATA_DIR".to_owned(), OsString::from("persistent-data")),
            (
                "PAB_DEPLOYMENT_ID".to_owned(),
                OsString::from("00000000-0000-0000-0000-000000000001"),
            ),
            (
                "PAB_TENANT_ID".to_owned(),
                OsString::from("00000000-0000-0000-0000-000000000002"),
            ),
            (
                "PAB_DEVICE_ID".to_owned(),
                OsString::from("00000000-0000-0000-0000-000000000003"),
            ),
            (
                "PAB_CONTROL_URL".to_owned(),
                OsString::from("wss://pab.example/control"),
            ),
            (
                "PAB_RELAY_URLS".to_owned(),
                OsString::from("https://relay-1.example, https://relay-2.example"),
            ),
            (
                "PAB_ENDPOINT_SECRET_FILE".to_owned(),
                OsString::from("endpoint.key"),
            ),
            (
                "PAB_DEVICE_CREDENTIAL_FILE".to_owned(),
                OsString::from("device-credential.json"),
            ),
        ])
    }

    #[test]
    fn parses_a_device_configuration_without_reading_process_globals() {
        let values = valid_values();
        let config = ExecutorConfig::from_lookup(|name| values.get(name).cloned()).unwrap();

        assert_eq!(config.device_id, DeviceId::from_u128(3));
        assert_eq!(config.operation_timeout, Duration::from_secs(10));
        assert_eq!(config.control_ca_cert, None);
        assert_eq!(
            config.task_database_file,
            PathBuf::from("persistent-data/executor.sqlite3")
        );
        assert_eq!(config.relay_urls.len(), 2);
    }

    #[test]
    fn default_identity_and_database_paths_survive_a_binary_move() {
        let mut values = valid_values();
        values.remove("PAB_ENDPOINT_SECRET_FILE");
        values.remove("PAB_DEVICE_CREDENTIAL_FILE");
        let config = ExecutorConfig::from_lookup(|name| values.get(name).cloned()).unwrap();
        assert_eq!(
            config.endpoint_secret_file,
            PathBuf::from("persistent-data/device-endpoint.key")
        );
        assert_eq!(
            config.device_credential_file,
            PathBuf::from("persistent-data/device-credential.json")
        );
        assert_eq!(
            config.task_database_file,
            PathBuf::from("persistent-data/executor.sqlite3")
        );
    }

    #[test]
    fn rejects_a_plaintext_control_url() {
        let mut values = valid_values();
        values.insert(
            "PAB_CONTROL_URL".to_owned(),
            OsString::from("ws://localhost/control"),
        );

        assert_eq!(
            ExecutorConfig::from_lookup(|name| values.get(name).cloned()).unwrap_err(),
            ExecutorConfigError::ControlUrlMustUseTls
        );
    }

    #[test]
    fn rejects_a_plaintext_relay_url() {
        let mut values = valid_values();
        values.insert(
            "PAB_RELAY_URLS".to_owned(),
            OsString::from("http://relay.example"),
        );

        assert!(matches!(
            ExecutorConfig::from_lookup(|name| values.get(name).cloned()),
            Err(ExecutorConfigError::RelayTlsRequired(_))
        ));
    }
}
