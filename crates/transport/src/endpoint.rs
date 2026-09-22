use std::{io::Cursor, time::Duration};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey, endpoint::presets,
};
use iroh_relay::tls::CaTlsConfig;
use thiserror::Error;

pub const PAB_ALPN: &[u8] = b"pixels-agent-bridge/1";

#[derive(Debug, Clone)]
pub struct PabEndpointConfig {
    relay_urls: Vec<RelayUrl>,
    tls: CaTlsConfig,
}

impl PabEndpointConfig {
    pub fn new(relay_urls: Vec<RelayUrl>) -> Result<Self, PabEndpointError> {
        if relay_urls.is_empty() {
            return Err(PabEndpointError::NoRelayUrls);
        }
        for relay_url in &relay_urls {
            if relay_url.scheme() != "https" {
                return Err(PabEndpointError::RelayTlsRequired(relay_url.to_string()));
            }
        }
        Ok(Self {
            relay_urls,
            tls: CaTlsConfig::embedded(),
        })
    }

    pub fn with_extra_ca_pem(mut self, pem: &[u8]) -> Result<Self, PabEndpointError> {
        let certificates =
            rustls_pemfile::certs(&mut Cursor::new(pem)).collect::<Result<Vec<_>, _>>()?;
        if certificates.is_empty() {
            return Err(PabEndpointError::NoCaCertificates);
        }
        self.tls = self.tls.with_extra_roots(certificates);
        Ok(self)
    }

    pub fn with_extra_ca_certificates(
        mut self,
        certificates: impl IntoIterator<Item = rustls_pki_types::CertificateDer<'static>>,
    ) -> Self {
        self.tls = self.tls.with_extra_roots(certificates);
        self
    }
}

pub struct PabEndpoint {
    inner: Endpoint,
}

impl PabEndpoint {
    pub async fn bind(
        config: PabEndpointConfig,
        secret: SecretKey,
    ) -> Result<Self, PabEndpointError> {
        let relay_map = RelayMap::from_iter(config.relay_urls);
        let inner = Endpoint::builder(presets::Minimal)
            .secret_key(secret)
            .alpns(vec![PAB_ALPN.to_vec()])
            .relay_mode(RelayMode::Custom(relay_map))
            .ca_tls_config(config.tls)
            .bind()
            .await
            .map_err(|error| PabEndpointError::Bind(error.to_string()))?;
        Ok(Self { inner })
    }

    pub fn id(&self) -> EndpointId {
        self.inner.id()
    }

    pub fn address(&self) -> EndpointAddr {
        self.inner.addr()
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.inner
    }

    pub async fn wait_online(&self, timeout: Duration) -> Result<EndpointAddr, PabEndpointError> {
        if timeout.is_zero() {
            return Err(PabEndpointError::InvalidOnlineTimeout);
        }
        tokio::time::timeout(timeout, self.inner.online())
            .await
            .map_err(|_| PabEndpointError::OnlineTimeout)?;
        Ok(self.address())
    }

    pub async fn close(&self) {
        self.inner.close().await;
    }
}

#[derive(Debug, Error)]
pub enum PabEndpointError {
    #[error("at least one self-hosted Relay URL must be configured")]
    NoRelayUrls,
    #[error("Relay URL must use HTTPS: {0}")]
    RelayTlsRequired(String),
    #[error("the Relay CA file contains no certificates")]
    NoCaCertificates,
    #[error("the Relay CA file is not valid PEM")]
    InvalidCaPem(#[from] std::io::Error),
    #[error("the iroh endpoint could not be bound: {0}")]
    Bind(String),
    #[error("the endpoint online timeout must be greater than zero")]
    InvalidOnlineTimeout,
    #[error("the endpoint did not connect to a self-hosted Relay before the timeout")]
    OnlineTimeout,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_an_explicit_tls_relay() {
        assert!(matches!(
            PabEndpointConfig::new(Vec::new()),
            Err(PabEndpointError::NoRelayUrls)
        ));
        let plaintext = "http://relay.example".parse().unwrap();
        assert!(matches!(
            PabEndpointConfig::new(vec![plaintext]),
            Err(PabEndpointError::RelayTlsRequired(_))
        ));
    }

    #[test]
    fn rejects_a_ca_file_without_certificates() {
        let relay = "https://relay.example".parse().unwrap();
        let error = PabEndpointConfig::new(vec![relay])
            .unwrap()
            .with_extra_ca_pem(b"not a certificate")
            .unwrap_err();
        assert!(matches!(error, PabEndpointError::NoCaCertificates));
    }
}
