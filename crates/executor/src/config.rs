use std::{env, ffi::OsString, path::PathBuf, time::Duration};

use pab_protocol::{DeploymentId, DeviceId, TenantId};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutorConfig {
    pub deployment_id: DeploymentId,
    pub tenant_id: TenantId,
    pub device_id: DeviceId,
    pub control_url: String,
    pub endpoint_secret_file: PathBuf,
    pub control_ca_cert: Option<PathBuf>,
    pub operation_timeout: Duration,
}

impl ExecutorConfig {
    pub fn from_env() -> Result<Self, ExecutorConfigError> {
        Self::from_lookup(|name| env::var_os(name))
    }

    fn from_lookup(
        mut lookup: impl FnMut(&str) -> Option<OsString>,
    ) -> Result<Self, ExecutorConfigError> {
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
            endpoint_secret_file: required_path(&mut lookup, "PAB_ENDPOINT_SECRET_FILE")?,
            control_ca_cert: lookup("PAB_CONTROL_CA_CERT").map(PathBuf::from),
            operation_timeout: Duration::from_secs(10),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ExecutorConfigError> {
        if !self.control_url.starts_with("wss://") {
            return Err(ExecutorConfigError::ControlUrlMustUseTls);
        }
        if self.operation_timeout.is_zero() {
            return Err(ExecutorConfigError::ZeroOperationTimeout);
        }
        Ok(())
    }
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

fn required_path(
    lookup: &mut impl FnMut(&str) -> Option<OsString>,
    name: &'static str,
) -> Result<PathBuf, ExecutorConfigError> {
    lookup(name)
        .map(PathBuf::from)
        .ok_or(ExecutorConfigError::Missing(name))
}

fn invalid(name: &'static str, error: impl std::fmt::Display) -> ExecutorConfigError {
    ExecutorConfigError::Invalid {
        name,
        message: error.to_string(),
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExecutorConfigError {
    #[error("{0} must be set")]
    Missing(&'static str),
    #[error("{0} must contain valid Unicode")]
    NotUnicode(&'static str),
    #[error("{name} is invalid: {message}")]
    Invalid { name: &'static str, message: String },
    #[error("PAB_CONTROL_URL must use wss://")]
    ControlUrlMustUseTls,
    #[error("the control operation timeout must be greater than zero")]
    ZeroOperationTimeout,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn valid_values() -> HashMap<String, OsString> {
        HashMap::from([
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
                "PAB_ENDPOINT_SECRET_FILE".to_owned(),
                OsString::from("endpoint.key"),
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
}
