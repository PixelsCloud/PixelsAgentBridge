use std::{env, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};

use pab_protocol::DeploymentId;
use thiserror::Error;

const MIN_CONTROL_SECRET_BYTES: usize = 32;

pub struct RelayServiceConfig {
    pub deployment_id: DeploymentId,
    pub control_url: String,
    pub control_secret: String,
    pub control_ca_cert: Option<PathBuf>,
    pub tls_cert: PathBuf,
    pub tls_key: PathBuf,
    pub https_bind: SocketAddr,
    pub captive_bind: SocketAddr,
    pub quic_bind: SocketAddr,
    pub policy_refresh_interval: Duration,
    pub reconnect_initial_delay: Duration,
    pub reconnect_max_delay: Duration,
    pub limiter_burst: Duration,
}

impl RelayServiceConfig {
    pub fn from_env() -> Result<Self, RelayServiceConfigError> {
        let config = Self {
            deployment_id: required("PAB_DEPLOYMENT_ID")?
                .parse()
                .map_err(|error| invalid("PAB_DEPLOYMENT_ID", error))?,
            control_url: required("PAB_CONTROL_URL")?,
            control_secret: required("PAB_RELAY_CONTROL_SECRET")?,
            control_ca_cert: env::var_os("PAB_CONTROL_CA_CERT").map(PathBuf::from),
            tls_cert: required_path("PAB_RELAY_TLS_CERT")?,
            tls_key: required_path("PAB_RELAY_TLS_KEY")?,
            https_bind: optional("PAB_RELAY_HTTPS_ADDR", "127.0.0.1:31443")?,
            captive_bind: "127.0.0.1:0".parse().expect("valid captive address"),
            quic_bind: optional("PAB_RELAY_QUIC_ADDR", "0.0.0.0:7842")?,
            policy_refresh_interval: Duration::from_secs(20),
            reconnect_initial_delay: Duration::from_secs(1),
            reconnect_max_delay: Duration::from_secs(30),
            limiter_burst: Duration::from_millis(100),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), RelayServiceConfigError> {
        if !self.control_url.starts_with("wss://") {
            return Err(RelayServiceConfigError::ControlUrlMustUseTls);
        }
        if self.control_secret.len() < MIN_CONTROL_SECRET_BYTES {
            return Err(RelayServiceConfigError::ControlSecretTooShort);
        }
        if !self.captive_bind.ip().is_loopback() {
            return Err(RelayServiceConfigError::CaptivePortalMustBeLoopback);
        }
        for (name, duration) in [
            ("policy refresh interval", self.policy_refresh_interval),
            ("initial reconnect delay", self.reconnect_initial_delay),
            ("maximum reconnect delay", self.reconnect_max_delay),
            ("limiter burst", self.limiter_burst),
        ] {
            if duration.is_zero() {
                return Err(RelayServiceConfigError::ZeroDuration(name));
            }
        }
        if self.reconnect_initial_delay > self.reconnect_max_delay {
            return Err(RelayServiceConfigError::InvalidReconnectRange);
        }
        Ok(())
    }
}

fn required(name: &'static str) -> Result<String, RelayServiceConfigError> {
    env::var(name).map_err(|_| RelayServiceConfigError::Missing(name))
}

fn required_path(name: &'static str) -> Result<PathBuf, RelayServiceConfigError> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or(RelayServiceConfigError::Missing(name))
}

fn optional<T>(name: &'static str, default: &str) -> Result<T, RelayServiceConfigError>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    env::var(name)
        .unwrap_or_else(|_| default.to_owned())
        .parse()
        .map_err(|error| invalid(name, error))
}

fn invalid(name: &'static str, error: impl std::fmt::Display) -> RelayServiceConfigError {
    RelayServiceConfigError::Invalid {
        name,
        message: error.to_string(),
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RelayServiceConfigError {
    #[error("{0} must be set")]
    Missing(&'static str),
    #[error("{name} is invalid: {message}")]
    Invalid { name: &'static str, message: String },
    #[error("PAB_CONTROL_URL must use wss://")]
    ControlUrlMustUseTls,
    #[error("PAB_RELAY_CONTROL_SECRET must contain at least 32 bytes")]
    ControlSecretTooShort,
    #[error("the iroh captive portal must remain bound to a loopback address")]
    CaptivePortalMustBeLoopback,
    #[error("{0} must be greater than zero")]
    ZeroDuration(&'static str),
    #[error("initial reconnect delay must not exceed maximum reconnect delay")]
    InvalidReconnectRange,
}
