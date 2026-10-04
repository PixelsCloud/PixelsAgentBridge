use std::{env, ffi::OsString, path::PathBuf, time::Duration};

use iroh_base::RelayUrl;
use pab_agent_core::{
    DataPathError, DataPaths, DataScope, OpenRegistrationKind, load_or_create_endpoint_secret,
    register_open_endpoint, tls_connector,
};
use pab_protocol::{EndpointKey, EndpointProofPrincipal, OperatorRef, TenantId, UserId};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeConfig {
    pub tenant_id: TenantId,
    pub identity: BridgeIdentity,
    pub control_url: String,
    pub relay_urls: Vec<RelayUrl>,
    pub endpoint_secret_file: PathBuf,
    pub control_ca_cert: Option<PathBuf>,
    pub relay_ca_cert: Option<PathBuf>,
    pub operation_timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeIdentity {
    Account(UserId),
    Guest,
}

impl BridgeIdentity {
    pub fn principal(self) -> EndpointProofPrincipal {
        match self {
            Self::Account(user_id) => EndpointProofPrincipal::User { user_id },
            Self::Guest => EndpointProofPrincipal::Guest,
        }
    }
    pub fn operator(self, endpoint_key: EndpointKey) -> OperatorRef {
        match self {
            Self::Account(user_id) => OperatorRef::account(user_id, endpoint_key),
            Self::Guest => OperatorRef::Guest {
                guest_endpoint_key: endpoint_key,
            },
        }
    }
}

impl BridgeConfig {
    pub async fn register_guest_from_env() -> Result<Self, GuestConfigError> {
        let paths = DataPaths::for_scope(DataScope::User)?;
        Self::register_guest_with_secret(paths.root().join("guest-endpoint.key")).await
    }

    /// Register the exact identity reserved by the caller. MCP processes use
    /// separate persistent slots; Desktop keeps its existing guest identity.
    pub async fn register_guest_with_secret(
        secret_path: PathBuf,
    ) -> Result<Self, GuestConfigError> {
        let secret = load_or_create_endpoint_secret(&secret_path)?;
        let control_url = env::var("PAB_CONTROL_URL")
            .map_err(|_| GuestConfigError::Missing("PAB_CONTROL_URL"))?;
        let ca = env::var_os("PAB_CONTROL_CA_CERT")
            .map(std::fs::read)
            .transpose()?;
        let connector = tls_connector(ca.as_deref())?;
        let result = loop {
            match register_open_endpoint(
                &control_url,
                &secret,
                OpenRegistrationKind::Guest,
                connector.clone(),
                Duration::from_secs(10),
            )
            .await
            {
                Ok(result) => break result,
                Err(
                    error @ (pab_agent_core::OpenRegistrationError::Timeout
                    | pab_agent_core::OpenRegistrationError::WebSocket(_)
                    | pab_agent_core::OpenRegistrationError::Server {
                        code: pab_protocol::ControlErrorCode::Internal,
                        ..
                    }),
                ) => {
                    tracing::warn!(%error, "guest registration unavailable; retrying in 3s");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
                Err(error) => return Err(error.into()),
            }
        };
        let pab_protocol::EndpointRegistrationResult::Guest { tenant_id, .. } = result else {
            return Err(GuestConfigError::InvalidResult);
        };
        Ok(Self::from_env_guest(tenant_id, secret_path)?)
    }

    pub fn from_env() -> Result<Self, BridgeConfigError> {
        Self::from_lookup(|name| env::var_os(name))
    }

    pub fn from_env_guest(
        tenant_id: TenantId,
        secret_file: PathBuf,
    ) -> Result<Self, BridgeConfigError> {
        Self::from_lookup(|name| match name {
            "PAB_GUEST" => Some(OsString::from("1")),
            "PAB_TENANT_ID" => Some(OsString::from(tenant_id.to_string())),
            "PAB_ENDPOINT_SECRET_FILE" => Some(secret_file.clone().into_os_string()),
            _ => env::var_os(name),
        })
    }

    pub fn from_env_account(
        tenant_id: TenantId,
        user_id: UserId,
        secret_file: PathBuf,
    ) -> Result<Self, BridgeConfigError> {
        Self::from_lookup(|name| match name {
            "PAB_GUEST" => Some(OsString::from("0")),
            "PAB_TENANT_ID" => Some(OsString::from(tenant_id.to_string())),
            "PAB_USER_ID" => Some(OsString::from(user_id.to_string())),
            "PAB_ENDPOINT_SECRET_FILE" => Some(secret_file.clone().into_os_string()),
            _ => env::var_os(name),
        })
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
            tenant_id: required_text(&mut lookup, "PAB_TENANT_ID")?
                .parse()
                .map_err(|error| invalid("PAB_TENANT_ID", error))?,
            identity: match lookup("PAB_GUEST") {
                Some(value) if value == "1" => BridgeIdentity::Guest,
                _ => BridgeIdentity::Account(
                    required_text(&mut lookup, "PAB_USER_ID")?
                        .parse()
                        .map_err(|error| invalid("PAB_USER_ID", error))?,
                ),
            },
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

#[derive(Debug, Error)]
pub enum GuestConfigError {
    #[error("{0} must be set")]
    Missing(&'static str),
    #[error("guest registration returned an unexpected result")]
    InvalidResult,
    #[error(transparent)]
    Config(#[from] BridgeConfigError),
    #[error(transparent)]
    DataPath(#[from] DataPathError),
    #[error(transparent)]
    Secret(#[from] pab_agent_core::EndpointSecretError),
    #[error(transparent)]
    Registration(#[from] pab_agent_core::OpenRegistrationError),
    #[error(transparent)]
    Tls(#[from] pab_agent_core::TlsConnectorError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
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
