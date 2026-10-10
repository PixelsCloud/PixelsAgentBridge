//! Server-owned configuration, loaded before any database writes.
use pab_service_config::{ConfigError, LogConfig, TlsConfig, load, resolve};
use serde::Deserialize;
use std::{net::SocketAddr, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub database: DatabaseConfig,
    pub listen: SocketAddr,
    pub tls: TlsConfig,
    pub web: WebConfig,
    pub relay: RelayConfig,
    pub github: Option<GithubSettings>,
    #[serde(default)]
    pub log: LogConfig,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    pub url: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebConfig {
    pub origin: String,
    #[serde(default = "enabled")]
    pub registration_enabled: bool,
    pub assets: std::path::PathBuf,
}
fn enabled() -> bool {
    true
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayConfig {
    pub control_secret: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubSettings {
    pub client_id: String,
    pub client_secret: String,
    pub proxy_url: Option<String>,
}
impl ServerConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let mut config: Self = load(path)?;
        config.validate()?;
        let base = path.parent().unwrap_or(Path::new("."));
        config.tls.resolve(base);
        resolve(base, &mut config.web.assets);
        resolve(base, &mut config.log.directory);
        Ok(config)
    }
    fn validate(&mut self) -> Result<(), ConfigError> {
        use ConfigError::Invalid;
        let database =
            url::Url::parse(&self.database.url).map_err(|_| Invalid("invalid database.url"))?;
        if !matches!(database.scheme(), "postgres" | "postgresql") || database.host_str().is_none()
        {
            return Err(Invalid("database.url requires a PostgreSQL URL"));
        }
        let origin =
            url::Url::parse(&self.web.origin).map_err(|_| Invalid("invalid web.origin"))?;
        if origin.scheme() != "https"
            || origin.host_str().is_none()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
            || !origin.username().is_empty()
            || origin.password().is_some()
        {
            return Err(Invalid(
                "web.origin requires an HTTPS origin without credentials or path",
            ));
        }
        self.web.origin = origin.origin().ascii_serialization();
        if !self
            .relay
            .control_secret
            .bytes()
            .all(|b| b.is_ascii_graphic())
        {
            return Err(Invalid(
                "relay.control_secret requires printable ASCII bytes",
            ));
        }
        crate::RelayControlAuth::new(&self.relay.control_secret)
            .map_err(|_| Invalid("relay.control_secret requires at least 32 bytes"))?;
        // Validate optional OAuth credentials and proxy before connecting to the database.
        self.github_config().map_err(Invalid)?;
        Ok(())
    }
    pub fn github_config(&self) -> Result<Option<crate::web::GithubConfig>, &'static str> {
        self.github
            .as_ref()
            .map(|g| {
                crate::web::GithubConfig::configured(
                    g.client_id.clone(),
                    g.client_secret.clone(),
                    &self.web.origin,
                    g.proxy_url.as_deref(),
                )
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn example() -> String {
        include_str!("../../../packaging/docker/pab-server.example.toml").into()
    }
    #[test]
    fn file_resolves_paths_and_rejects_invalid_settings_without_secrets() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("server.toml");
        let valid = example().replace("/run/pab-certs/backend-cert.pem", "cert.pem");
        std::fs::write(&path, &valid).unwrap();
        let config = ServerConfig::load(&path).unwrap();
        assert_eq!(config.tls.cert, root.path().join("cert.pem"));
        for invalid in [
            valid.replace("[relay]", "[relay]\ndefault_user_mbps = 10"),
            valid.replace("[relay]", "[relay]\ndefault_guest_mbps = 10"),
            valid.replace("https://bridge.example.com", "http://bridge.example.com"),
            valid.replace("[relay]", "[relay]\ndefault_user_mbps = 'private-secret'"),
        ] {
            std::fs::write(&path, invalid).unwrap();
            let err = ServerConfig::load(&path).err().unwrap().to_string();
            assert!(!err.contains("private-secret"));
        }
    }
}
