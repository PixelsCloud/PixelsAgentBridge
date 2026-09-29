use std::{io::Cursor, net::SocketAddr, time::Duration};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey, Watcher as _,
    endpoint::presets,
};
use iroh_relay::tls::CaTlsConfig;
use n0_future::StreamExt;
use thiserror::Error;
use tokio::sync::watch;

use crate::PabConnection;

pub const PAB_ALPN: &[u8] = b"pixels-agent-bridge/1";

#[derive(Debug, Clone)]
pub struct PabEndpointConfig {
    relay_urls: Vec<RelayUrl>,
    tls: CaTlsConfig,
    #[cfg(debug_assertions)]
    relay_only_for_testing: bool,
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
            #[cfg(debug_assertions)]
            relay_only_for_testing: false,
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

    #[cfg(debug_assertions)]
    pub fn relay_only_for_testing(mut self) -> Self {
        self.relay_only_for_testing = true;
        self
    }
}

#[derive(Clone)]
pub struct PabEndpoint {
    inner: Endpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PabEndpointAddress {
    pub relay_urls: Vec<String>,
    pub direct_addresses: Vec<SocketAddr>,
}

impl PabEndpoint {
    pub async fn bind(
        config: PabEndpointConfig,
        secret: SecretKey,
    ) -> Result<Self, PabEndpointError> {
        let relay_map = RelayMap::from_iter(config.relay_urls);
        let builder = Endpoint::builder(presets::Minimal)
            .secret_key(secret)
            .alpns(vec![PAB_ALPN.to_vec()])
            .relay_mode(RelayMode::Custom(relay_map))
            .ca_tls_config(config.tls);
        #[cfg(debug_assertions)]
        let builder = if config.relay_only_for_testing {
            builder.clear_ip_transports()
        } else {
            builder
        };
        let inner = builder
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

    pub fn watch_address(&self) -> watch::Receiver<PabEndpointAddress> {
        let (sender, receiver) = watch::channel(convert_address(&self.inner.addr()));
        let mut addresses = self.inner.watch_addr().stream();
        let closed = self.inner.closed();
        tokio::spawn(closed.run_until(async move {
            while let Some(address) = addresses.next().await {
                if sender.send(convert_address(&address)).is_err() {
                    break;
                }
            }
        }));
        receiver
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.inner
    }

    pub async fn connect(
        &self,
        endpoint_key: [u8; 32],
        address: &PabEndpointAddress,
        timeout: Duration,
    ) -> Result<PabConnection, PabEndpointError> {
        if timeout.is_zero() {
            return Err(PabEndpointError::InvalidConnectionTimeout);
        }
        let endpoint_id = EndpointId::from_bytes(&endpoint_key)
            .map_err(|error| PabEndpointError::InvalidEndpointKey(error.to_string()))?;
        let mut endpoint_address = EndpointAddr::new(endpoint_id);
        for relay_url in &address.relay_urls {
            let relay_url = relay_url
                .parse::<RelayUrl>()
                .map_err(|error| PabEndpointError::InvalidRemoteRelay(error.to_string()))?;
            if relay_url.scheme() != "https" {
                return Err(PabEndpointError::RelayTlsRequired(relay_url.to_string()));
            }
            endpoint_address = endpoint_address.with_relay_url(relay_url);
        }
        for direct_address in &address.direct_addresses {
            endpoint_address = endpoint_address.with_ip_addr(*direct_address);
        }
        let connection =
            tokio::time::timeout(timeout, self.inner.connect(endpoint_address, PAB_ALPN))
                .await
                .map_err(|_| PabEndpointError::ConnectionTimeout)?
                .map_err(|error| PabEndpointError::Connect(error.to_string()))?;
        Ok(PabConnection::new(connection))
    }

    pub async fn accept(&self) -> Result<Option<PabConnection>, PabEndpointError> {
        let Some(incoming) = self.inner.accept().await else {
            return Ok(None);
        };
        let connection = incoming
            .await
            .map_err(|error| PabEndpointError::Accept(error.to_string()))?;
        Ok(Some(PabConnection::new(connection)))
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

fn convert_address(address: &EndpointAddr) -> PabEndpointAddress {
    PabEndpointAddress {
        relay_urls: address.relay_urls().map(ToString::to_string).collect(),
        direct_addresses: address.ip_addrs().copied().collect(),
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
    #[error("PAB connection timeout must be greater than zero")]
    InvalidConnectionTimeout,
    #[error("remote Endpoint key is invalid: {0}")]
    InvalidEndpointKey(String),
    #[error("remote Relay URL is invalid: {0}")]
    InvalidRemoteRelay(String),
    #[error("PAB connection attempt timed out")]
    ConnectionTimeout,
    #[error("PAB connection failed: {0}")]
    Connect(String),
    #[error("PAB incoming connection failed: {0}")]
    Accept(String),
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

    #[test]
    fn converts_only_supported_address_types() {
        let endpoint_id = SecretKey::generate().public();
        let relay = "https://relay.example".parse().unwrap();
        let direct = "192.0.2.8:7842".parse().unwrap();
        let address = EndpointAddr::new(endpoint_id)
            .with_relay_url(relay)
            .with_ip_addr(direct);

        assert_eq!(
            convert_address(&address),
            PabEndpointAddress {
                relay_urls: vec!["https://relay.example/".to_owned()],
                direct_addresses: vec![direct],
            }
        );
    }
}
