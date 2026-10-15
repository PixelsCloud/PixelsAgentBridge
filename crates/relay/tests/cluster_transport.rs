//! Real TLS Relay-only probes. No public servers, production identities or skipped TLS checks.
use std::{
    net::Ipv4Addr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use iroh_base::{EndpointId, RelayUrl, SecretKey};
use iroh_relay::server::{
    CertConfig, ForwardingControl, ForwardingDecision, RelayConfig, Server, ServerConfig,
    TlsConfig, testing,
};
use pab_transport::{ConnectionPath, PabEndpoint, PabEndpointAddress, PabEndpointConfig};

#[derive(Debug, Default)]
struct Traffic(AtomicU64);
impl ForwardingControl for Traffic {
    fn check(&self, _: EndpointId, _: EndpointId, _: usize) -> ForwardingDecision {
        ForwardingDecision::Allow
    }
    fn on_forwarded(&self, _: EndpointId, _: EndpointId, bytes: usize) {
        self.0.fetch_add(bytes as u64, Ordering::Relaxed);
    }
}

struct Node {
    server: Server,
    url: RelayUrl,
    traffic: Arc<Traffic>,
}
async fn node(tls: rustls::ServerConfig) -> Node {
    let traffic = Arc::new(Traffic::default());
    let mut relay = RelayConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.tls = Some(TlsConfig::new(
        (Ipv4Addr::LOCALHOST, 0),
        CertConfig::Manual { server_config: tls },
    ));
    relay.forwarding = traffic.clone();
    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    let server = Server::spawn(config).await.unwrap();
    let url = format!("https://localhost:{}", server.https_addr().unwrap().port())
        .parse()
        .unwrap();
    Node {
        server,
        url,
        traffic,
    }
}

async fn at(endpoint: &PabEndpoint, url: &RelayUrl) {
    let mut addresses = endpoint.watch_address();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if addresses.borrow().relay_urls == [url.to_string()] {
                break;
            }
            addresses.changed().await.unwrap();
        }
    })
    .await
    .expect("home Relay updated within the bound");
}

async fn exchange(a: &PabEndpoint, b: &PabEndpoint, marker: u8) {
    let address = PabEndpointAddress {
        relay_urls: b.address().relay_urls().map(ToString::to_string).collect(),
        direct_addresses: vec![],
    };
    let (client, server) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(
            a.connect(*b.id().as_bytes(), &address, Duration::from_secs(15)),
            b.accept()
        )
    })
    .await
    .expect("bounded dial/accept");
    let client = client.unwrap();
    let server = server.unwrap().unwrap();
    let payload: Vec<u8> = (0..12_000)
        .map(|i| (i as u8).wrapping_add(marker))
        .collect();
    tokio::time::timeout(Duration::from_secs(15), async {
        let send = async {
            let mut stream = client.open_bi(Duration::from_secs(5)).await.unwrap();
            stream
                .send_json(&payload, Duration::from_secs(5))
                .await
                .unwrap();
            let echoed: Vec<u8> = stream.receive_json(Duration::from_secs(5)).await.unwrap();
            assert_eq!(echoed, payload);
        };
        let echo = async {
            let mut stream = server.accept_bi(Duration::from_secs(5)).await.unwrap();
            let bytes: Vec<u8> = stream.receive_json(Duration::from_secs(5)).await.unwrap();
            assert_eq!(bytes, payload);
            stream
                .send_json(&bytes, Duration::from_secs(5))
                .await
                .unwrap();
        };
        tokio::join!(send, echo);
    })
    .await
    .unwrap();
    assert_eq!(client.selected_path(), Some(ConnectionPath::Relay));
    assert_eq!(server.selected_path(), Some(ConnectionPath::Relay));
    client.close(b"probe completed");
    server.close(b"probe completed");
}

#[tokio::test]
async fn different_home_relays_and_live_directory_replacement_preserve_identity() {
    let (certs, tls) = testing::self_signed_tls_certs_and_config();
    let first = node(tls.clone()).await;
    let second = node(tls.clone()).await;
    let config = |url: &RelayUrl| {
        PabEndpointConfig::new(vec![url.clone()])
            .unwrap()
            .with_extra_ca_certificates(certs.clone())
            .relay_only_for_testing()
    };
    let a = PabEndpoint::bind(config(&first.url), SecretKey::from_bytes(&[191; 32]))
        .await
        .unwrap();
    let b = PabEndpoint::bind(config(&second.url), SecretKey::from_bytes(&[192; 32]))
        .await
        .unwrap();
    at(&a, &first.url).await;
    at(&b, &second.url).await;
    let identities = (a.id(), b.id());
    exchange(&a, &b, 1).await;
    exchange(&b, &a, 2).await;
    // iroh deliberately reuses a learned reverse route. A fresh peer is needed
    // to exercise A independently; reverse dialing alone can keep using B.
    let fresh = PabEndpoint::bind(config(&second.url), SecretKey::from_bytes(&[193; 32]))
        .await
        .unwrap();
    at(&fresh, &second.url).await;
    exchange(&fresh, &a, 5).await;
    fresh.close().await;
    assert!(first.traffic.0.load(Ordering::Relaxed) > 0);
    assert!(second.traffic.0.load(Ordering::Relaxed) > 0);

    // Add before removing; use the very same endpoint, keys and address watcher.
    assert!(a.update_relays(1, &[second.url.to_string()]).await.unwrap());
    assert!(!a.update_relays(1, &[first.url.to_string()]).await.unwrap());
    assert!(
        a.update_relays(2, &["http://untrusted.invalid".into()])
            .await
            .is_err()
    );
    at(&a, &second.url).await;
    first.server.shutdown().await.unwrap();
    let baseline = second.traffic.0.load(Ordering::Relaxed);
    exchange(&a, &b, 3).await;
    assert!(second.traffic.0.load(Ordering::Relaxed) > baseline);
    assert_eq!((a.id(), b.id()), identities);

    let third = node(tls).await;
    for endpoint in [&a, &b] {
        endpoint
            .update_relays(3, &[third.url.to_string()])
            .await
            .unwrap();
    }
    at(&a, &third.url).await;
    at(&b, &third.url).await;
    second.server.shutdown().await.unwrap();
    exchange(&b, &a, 4).await;
    assert!(third.traffic.0.load(Ordering::Relaxed) > 0);
    assert_eq!((a.id(), b.id()), identities);
    a.close().await;
    b.close().await;
    third.server.shutdown().await.unwrap();
}

#[tokio::test]
async fn selects_known_backup_after_home_relay_dies() {
    let (certs, tls) = testing::self_signed_tls_certs_and_config();
    let first = node(tls.clone()).await;
    let second = node(tls).await;
    let config = || {
        PabEndpointConfig::new(vec![first.url.clone()])
            .unwrap()
            .with_extra_ca_certificates(certs.clone())
            .relay_only_for_testing()
    };
    let a = PabEndpoint::bind(config(), SecretKey::from_bytes(&[194; 32]))
        .await
        .unwrap();
    let b = PabEndpoint::bind(config(), SecretKey::from_bytes(&[195; 32]))
        .await
        .unwrap();
    at(&a, &first.url).await;
    at(&b, &first.url).await;
    exchange(&a, &b, 6).await;
    for endpoint in [&a, &b] {
        endpoint
            .update_relays(1, &[first.url.to_string(), second.url.to_string()])
            .await
            .unwrap();
    }
    first.server.shutdown().await.unwrap();
    // No directory removal/maintenance notification: exercise abrupt failure.
    at(&a, &second.url).await;
    at(&b, &second.url).await;
    exchange(&a, &b, 7).await;
    assert!(second.traffic.0.load(Ordering::Relaxed) > 0);
    a.close().await;
    b.close().await;
    second.server.shutdown().await.unwrap();
}

#[tokio::test]
async fn directory_controller_uses_cached_backup_without_server_notification() {
    let (certs, tls) = testing::self_signed_tls_certs_and_config();
    let first = node(tls.clone()).await;
    let second = node(tls).await;
    let config = |url: &RelayUrl| {
        PabEndpointConfig::new(vec![url.clone()])
            .unwrap()
            .with_extra_ca_certificates(certs.clone())
            .relay_only_for_testing()
    };
    let a = PabEndpoint::bind(config(&first.url), SecretKey::from_bytes(&[196; 32]))
        .await
        .unwrap();
    let b = PabEndpoint::bind(config(&second.url), SecretKey::from_bytes(&[197; 32]))
        .await
        .unwrap();
    let (_keep_catalog, updates) = tokio::sync::watch::channel((
        1,
        vec![
            first.url.as_str().trim_end_matches('/').to_owned(),
            second.url.to_string(),
        ],
    ));
    a.follow_relay_updates(updates);
    at(&a, &first.url).await;
    at(&b, &second.url).await;
    exchange(&a, &b, 8).await;
    first.server.shutdown().await.unwrap();
    at(&a, &second.url).await;
    exchange(&a, &b, 9).await;
    a.close().await;
    b.close().await;
    second.server.shutdown().await.unwrap();
}

#[tokio::test]
async fn twenty_directory_switches_keep_endpoints_and_transfer_contents() {
    let (certs, tls) = testing::self_signed_tls_certs_and_config();
    let first = node(tls.clone()).await;
    let second = node(tls).await;
    let config = || {
        PabEndpointConfig::new(vec![first.url.clone()])
            .unwrap()
            .with_extra_ca_certificates(certs.clone())
            .relay_only_for_testing()
    };
    let a = PabEndpoint::bind(config(), SecretKey::from_bytes(&[198; 32]))
        .await
        .unwrap();
    let b = PabEndpoint::bind(config(), SecretKey::from_bytes(&[199; 32]))
        .await
        .unwrap();
    let identities = (a.id(), b.id());
    let (directory, updates) = tokio::sync::watch::channel((1, vec![first.url.to_string()]));
    a.follow_relay_updates(updates.clone());
    b.follow_relay_updates(updates);
    at(&a, &first.url).await;
    at(&b, &first.url).await;
    exchange(&a, &b, 0).await;
    for index in 0..20u64 {
        let target = if index % 2 == 0 { &second } else { &first };
        directory.send_replace((index + 2, vec![target.url.to_string()]));
        at(&a, &target.url).await;
        at(&b, &target.url).await;
        exchange(&a, &b, index as u8 + 1).await;
        assert_eq!((a.id(), b.id()), identities);
    }
    assert!(first.traffic.0.load(Ordering::Relaxed) > 0);
    assert!(second.traffic.0.load(Ordering::Relaxed) > 0);
    a.close().await;
    b.close().await;
    first.server.shutdown().await.unwrap();
    second.server.shutdown().await.unwrap();
}
