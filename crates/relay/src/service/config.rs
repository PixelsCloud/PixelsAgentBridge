use std::{env, fs, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};
use thiserror::Error;

const MIN_CONTROL_SECRET_BYTES: usize = 32;

pub struct RelayServiceConfig {
    pub control_url: String,
    pub control_secret: String,
    pub control_ca_cert: Option<PathBuf>,
    pub tls_cert: PathBuf,
    pub tls_key: PathBuf,
    pub https_bind: SocketAddr,
    pub captive_bind: SocketAddr,
    pub quic_bind: SocketAddr,
    pub policy_refresh_interval: Duration,
    pub reconnect_interval: Duration,
    pub limiter_burst: Duration,
}

impl RelayServiceConfig {
    pub fn from_env() -> Result<Self, RelayServiceConfigError> {
        let config = Self {
            control_url: required("PAB_CONTROL_URL")?,
            control_secret: required_secret("PAB_RELAY_CONTROL_SECRET")?,
            control_ca_cert: env::var_os("PAB_CONTROL_CA_CERT").map(PathBuf::from),
            tls_cert: required_path("PAB_RELAY_TLS_CERT")?,
            tls_key: required_path("PAB_RELAY_TLS_KEY")?,
            https_bind: optional("PAB_RELAY_HTTPS_ADDR", "127.0.0.1:31443")?,
            captive_bind: "127.0.0.1:0".parse().expect("valid captive address"),
            quic_bind: optional("PAB_RELAY_QUIC_ADDR", "0.0.0.0:7842")?,
            policy_refresh_interval: Duration::from_secs(20),
            reconnect_interval: Duration::from_secs(3),
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
            ("reconnect interval", self.reconnect_interval),
            ("limiter burst", self.limiter_burst),
        ] {
            if duration.is_zero() {
                return Err(RelayServiceConfigError::ZeroDuration(name));
            }
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

fn required_secret(name: &'static str) -> Result<String, RelayServiceConfigError> {
    if let Ok(value) = env::var(name) {
        return Ok(value);
    }
    let file_name = format!("{name}_FILE");
    let path = env::var_os(&file_name).map(PathBuf::from).ok_or_else(|| {
        RelayServiceConfigError::MissingSecretFile {
            name,
            file_name: file_name.clone(),
        }
    })?;
    let value = fs::read_to_string(path).map_err(|error| RelayServiceConfigError::SecretFile {
        file_name: file_name.clone(),
        message: error.to_string(),
    })?;
    let value = value.trim_end_matches(['\r', '\n']).to_owned();
    if value.is_empty() {
        return Err(RelayServiceConfigError::SecretFile {
            file_name,
            message: "file is empty".to_owned(),
        });
    }
    Ok(value)
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
    #[error("{name} or {file_name} must be set")]
    MissingSecretFile {
        name: &'static str,
        file_name: String,
    },
    #[error("{file_name} could not be read: {message}")]
    SecretFile { file_name: String, message: String },
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
}
