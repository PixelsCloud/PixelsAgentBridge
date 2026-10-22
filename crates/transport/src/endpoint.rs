use std::{io::Cursor, net::SocketAddr, sync::Arc, time::Duration};

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionMode {
    #[default]
    Auto,
    Relay,
}

#[derive(Debug, Clone)]
pub struct PabEndpointConfig {
    relay_urls: Vec<RelayUrl>,
    tls: CaTlsConfig,
    #[cfg(any(debug_assertions, feature = "test-utils"))]
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
        let config = Self {
            relay_urls,
            tls: CaTlsConfig::embedded(),
            #[cfg(any(debug_assertions, feature = "test-utils"))]
            relay_only_for_testing: false,
        };
        let private_ca = pab_service_config::client::DEFAULT_RELAY_CA_PEM.as_bytes();
        if private_ca.is_empty() {
            Ok(config)
        } else {
            config.with_extra_ca_pem(private_ca)
        }
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

    #[cfg(any(debug_assertions, feature = "test-utils"))]
    pub fn relay_only_for_testing(mut self) -> Self {
        self.relay_only_for_testing = true;
        self
    }
}

#[derive(Clone)]
pub struct PabEndpoint {
    inner: Endpoint,
    relay_map: RelayMap,
    relay_revision: Arc<tokio::sync::Mutex<u64>>,
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
            .relay_mode(RelayMode::Custom(relay_map.clone()))
            .ca_tls_config(config.tls);
        #[cfg(any(debug_assertions, feature = "test-utils"))]
        let builder = if config.relay_only_for_testing {
            builder.clear_ip_transports()
        } else {
            builder
        };
        let inner = builder
            .bind()
            .await
            .map_err(|error| PabEndpointError::Bind(error.to_string()))?;
        Ok(Self {
            inner,
            relay_map,
            relay_revision: Arc::new(tokio::sync::Mutex::new(0)),
        })
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

    /// Follows an authenticated control supervisor. The task dies with either
    /// the endpoint or the producer; it never owns a second control connection.
    pub fn follow_relay_updates(&self, mut updates: watch::Receiver<(u64, Vec<String>)>) {
        let endpoint = self.clone();
        let closed = self.inner.closed();
        tokio::spawn(closed.run_until(async move {
            let mut catalog_revision = 0;
            let mut mutation_revision = *endpoint.relay_revision.lock().await;
            let mut candidates = Vec::<String>::new();
            let mut selected = None::<String>;
            let mut failing_since = None::<tokio::time::Instant>;
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut first = true;
            loop {
                if !first {
                    tokio::select! {
                        changed=updates.changed()=>if changed.is_err(){break;},
                        _=tick.tick(),if catalog_revision>0=>{},
                    }
                }
                first = false;
                let (revision, urls) = updates.borrow_and_update().clone();
                if revision > catalog_revision {
                    // Validate the complete candidate set before considering a switch.
                    if validate_relay_urls(&urls).is_err() {
                        continue;
                    }
                    // TOML commonly omits the trailing slash; iroh exposes a
                    // canonical RelayUrl. Compare canonical values, otherwise
                    // a healthy node appears disconnected and rotates forever.
                    candidates = urls
                        .into_iter()
                        .filter_map(|value| value.parse::<RelayUrl>().ok())
                        .map(|url| url.to_string())
                        .collect();
                    catalog_revision = revision;
                }
                if catalog_revision == 0 {
                    continue;
                }
                let mut next = selected
                    .clone()
                    .filter(|url| candidates.contains(url))
                    .or_else(|| candidates.first().cloned());
                let healthy = endpoint
                    .inner
                    .home_relay_status()
                    .get()
                    .iter()
                    .any(|status| {
                        status.is_connected() && next.as_deref() == Some(status.url().as_str())
                    });
                if healthy {
                    failing_since = None;
                } else if let Some(url) = &next {
                    let since = failing_since.get_or_insert_with(tokio::time::Instant::now);
                    let jitter = u64::from(endpoint.id().as_bytes()[0]) * 4;
                    if since.elapsed() >= Duration::from_millis(3000 + jitter)
                        && candidates.len() > 1
                    {
                        let index = candidates
                            .iter()
                            .position(|candidate| candidate == url)
                            .unwrap_or(0);
                        next = Some(candidates[(index + 1) % candidates.len()].clone());
                        failing_since = None;
                    }
                }
                if next != selected || mutation_revision == 0 {
                    mutation_revision = mutation_revision.saturating_add(1);
                    let desired: Vec<_> = next.clone().into_iter().collect();
                    if let Err(error) = endpoint.update_relays(mutation_revision, &desired).await {
                        tracing::warn!(%error, revision, "Relay directory could not be applied");
                    } else {
                        selected = next;
                        failing_since = None;
                    }
                }
            }
        }));
    }

    /// Apply a complete authenticated directory's eligible URL set. Validate
    /// everything before mutation; serialize revisions and add before removing.
    /// An empty set is legitimate during an outage and does not close P2P.
    pub async fn update_relays(
        &self,
        revision: u64,
        urls: &[String],
    ) -> Result<bool, PabEndpointError> {
        if revision == 0 {
            return Err(PabEndpointError::InvalidRelayDirectory);
        }
        let desired = validate_relay_urls(urls)?;
        let mut current = self.relay_revision.lock().await;
        if revision <= *current {
            return Ok(false);
        }
        if self.inner.is_closed() {
            return Err(PabEndpointError::EndpointClosed);
        }
        let previous = self.relay_map.urls::<Vec<_>>();
        for url in &desired {
            if !previous.contains(url) {
                self.inner
                    .insert_relay(
                        url.clone(),
                        Arc::new(iroh_relay::RelayConfig::from(url.clone())),
                    )
                    .await;
            }
        }
        for url in previous {
            if !desired.contains(&url) {
                self.inner.remove_relay(&url).await;
            }
        }
        if self.inner.is_closed() {
            return Err(PabEndpointError::EndpointClosed);
        }
        *current = revision;
        Ok(true)
    }

    pub async fn connect(
        &self,
        endpoint_key: [u8; 32],
        address: &PabEndpointAddress,
        timeout: Duration,
    ) -> Result<PabConnection, PabEndpointError> {
        self.connect_with_mode(endpoint_key, address, timeout, ConnectionMode::Auto).await
    }

    pub async fn connect_with_mode(
        &self,
        endpoint_key: [u8; 32],
        address: &PabEndpointAddress,
        timeout: Duration,
        mode: ConnectionMode,
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
        if mode == ConnectionMode::Relay && address.relay_urls.is_empty() {
            return Err(PabEndpointError::NoRemoteRelay);
        }
        if mode == ConnectionMode::Auto {
            for direct_address in &address.direct_addresses {
                endpoint_address = endpoint_address.with_ip_addr(*direct_address);
            }
        }
        let connection =
            tokio::time::timeout(timeout, async {
                self.inner.connect_with_opts(endpoint_address, PAB_ALPN,
                    iroh::endpoint::ConnectOptions::new().with_relay_only(mode == ConnectionMode::Relay))
                    .await.map_err(|e| e.to_string())?.await.map_err(|e| e.to_string())
            })
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

fn validate_relay_urls(
    urls: &[String],
) -> Result<std::collections::BTreeSet<RelayUrl>, PabEndpointError> {
    if urls.len() > 64 {
        return Err(PabEndpointError::InvalidRelayDirectory);
    }
    let mut desired = std::collections::BTreeSet::new();
    for value in urls {
        let url = value
            .parse::<RelayUrl>()
            .map_err(|_| PabEndpointError::InvalidRelayDirectory)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || url.path() != "/"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || value.len() > 2048
            || !desired.insert(url)
        {
            return Err(PabEndpointError::InvalidRelayDirectory);
        }
    }
    Ok(desired)
}

fn convert_address(address: &EndpointAddr) -> PabEndpointAddress {
    PabEndpointAddress {
        relay_urls: address.relay_urls().map(ToString::to_string).collect(),
        direct_addresses: address.ip_addrs().copied().collect(),
    }
}

#[derive(Debug, Error)]
pub enum PabEndpointError {
    #[error("relay_connection_unavailable: the remote device has no Relay address; direct fallback is disabled")]
    NoRemoteRelay,
    #[error("invalid Relay directory")]
    InvalidRelayDirectory,
    #[error("endpoint is closed")]
    EndpointClosed,
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
