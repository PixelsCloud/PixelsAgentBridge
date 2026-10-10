use pab_service_config::{ConfigError, LogConfig, TlsConfig, load, resolve};
use serde::Deserialize;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};
use thiserror::Error;

const MIN_CONTROL_SECRET_BYTES: usize = 32;

pub struct RelayServiceConfig {
    pub node_id: String,
    pub usage_dir: PathBuf,
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
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RelayServiceConfigError {
    #[error("control.url must use wss://")]
    ControlUrlMustUseTls,
    #[error("control.secret must contain at least 32 bytes")]
    ControlSecretTooShort,
    #[error("the iroh captive portal must remain bound to a loopback address")]
    CaptivePortalMustBeLoopback,
    #[error("{0} must be greater than zero")]
    ZeroDuration(&'static str),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayFileConfig {
    pub node_id: String,
    pub usage_dir: PathBuf,
    pub https_bind: SocketAddr,
    pub quic_bind: SocketAddr,
    pub tls: TlsConfig,
    pub control: ControlConfig,
    #[serde(default)]
    pub log: LogConfig,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlConfig {
    pub url: String,
    pub secret: String,
    pub ca_cert: Option<PathBuf>,
}
impl RelayFileConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let mut config: Self = load(path)?;
        let valid_url = config
            .control
            .url
            .parse::<tokio_tungstenite::tungstenite::http::Uri>()
            .ok()
            .is_some_and(|u| {
                u.scheme_str() == Some("wss")
                    && u.host().is_some()
                    && !u.authority().is_some_and(|a| a.as_str().contains('@'))
            });
        if !valid_url {
            return Err(ConfigError::Invalid(
                "control.url requires a wss:// URL without credentials",
            ));
        }
        if config.control.secret.len() < MIN_CONTROL_SECRET_BYTES
            || !config.control.secret.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(ConfigError::Invalid(
                "control.secret requires at least 32 printable ASCII bytes",
            ));
        }
        if config.node_id.is_empty()
            || config.node_id.len() > 64
            || !config
                .node_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err(ConfigError::Invalid(
                "node_id requires 1-64 letters, digits, dots, underscores or hyphens",
            ));
        }
        let base = path.parent().unwrap_or(Path::new("."));
        config.tls.resolve(base);
        resolve(base, &mut config.usage_dir);
        resolve(base, &mut config.log.directory);
        if let Some(ca) = &mut config.control.ca_cert {
            resolve(base, ca);
        }
        Ok(config)
    }
    pub fn into_service(self) -> RelayServiceConfig {
        RelayServiceConfig {
            node_id: self.node_id,
            usage_dir: self.usage_dir,
            control_url: self.control.url,
            control_secret: self.control.secret,
            control_ca_cert: self.control.ca_cert,
            tls_cert: self.tls.cert,
            tls_key: self.tls.key,
            https_bind: self.https_bind,
            quic_bind: self.quic_bind,
            captive_bind: "127.0.0.1:0".parse().expect("fixed loopback address"),
            policy_refresh_interval: Duration::from_secs(20),
            reconnect_interval: Duration::from_secs(3),
            limiter_burst: Duration::from_millis(100),
        }
    }
}
#[cfg(test)]
mod file_tests {
    use super::*;
    #[test]
    fn relay_file_passes_node_and_storage_and_validates_secrets() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("relay.toml");
        let valid = include_str!("../../../../packaging/docker/pab-relay-server.example.toml")
            .replace("/var/lib/pab/usage", "usage");
        std::fs::write(&path, &valid).unwrap();
        let runtime = RelayFileConfig::load(&path).unwrap().into_service();
        runtime.validate().unwrap();
        assert_eq!(runtime.node_id, "primary");
        assert_eq!(runtime.usage_dir, root.path().join("usage"));
        for invalid in [
            valid.replace("wss://", "ws://"),
            valid.replace("primary", "bad node"),
            valid.replace(
                "replace-with-a-random-secret-at-least-32-bytes",
                "private-secret",
            ),
        ] {
            std::fs::write(&path, invalid).unwrap();
            let err = RelayFileConfig::load(&path).err().unwrap().to_string();
            assert!(!err.contains("private-secret"));
        }
    }
}
