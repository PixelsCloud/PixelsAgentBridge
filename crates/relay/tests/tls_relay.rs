use std::{
    collections::HashSet,
    net::Ipv4Addr,
    sync::atomic::{AtomicUsize, Ordering},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use iroh_base::{EndpointId, RelayUrl, SecretKey};
use iroh_dns::dns::DnsResolver;
use iroh_relay::{
    client::ClientBuilder,
    protos::relay::{ClientToRelayMsg, Datagrams, RelayToClientMsg},
    server::{
        Access, AccessControl, CertConfig, ClientRequest, ForwardingControl, ForwardingDecision,
        RelayConfig, Server, ServerConfig, TlsConfig, testing,
    },
    tls::{CaTlsConfig, default_provider},
};
use n0_future::{SinkExt, StreamExt};
use pab_transport::{PabEndpoint, PabEndpointConfig};
use rustls::{ClientConfig, RootCertStore, pki_types::ServerName};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::TlsConnector;

#[derive(Debug)]
struct EndpointAllowlist {
    allowed: HashSet<EndpointId>,
    seen: Mutex<Vec<EndpointId>>,
}

impl AccessControl for EndpointAllowlist {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        let endpoint_id = request.endpoint_id();
        self.seen.lock().expect("seen lock").push(endpoint_id);
        if self.allowed.contains(&endpoint_id) {
            Access::Allow
        } else {
            Access::Deny {
                reason: Some("endpoint is not registered".to_owned()),
            }
        }
    }
}

#[derive(Debug, Default)]
struct DelayFirstDatagram {
    calls: AtomicUsize,
    endpoints: Mutex<Vec<(EndpointId, EndpointId, usize)>>,
}

impl ForwardingControl for DelayFirstDatagram {
    fn check(&self, src: EndpointId, dst: EndpointId, bytes: usize) -> ForwardingDecision {
        self.endpoints
            .lock()
            .expect("endpoint lock")
            .push((src, dst, bytes));
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            ForwardingDecision::Wait(Duration::from_millis(60))
        } else {
            ForwardingDecision::Allow
        }
    }
}

#[tokio::test]
async fn relay_serves_health_over_trusted_self_signed_tls() {
    let (certificates, server_tls) = testing::self_signed_tls_certs_and_config();
    let mut relay = RelayConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.tls = Some(TlsConfig::new(
        (Ipv4Addr::LOCALHOST, 0),
        CertConfig::Manual {
            server_config: server_tls,
        },
    ));

    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    let server = Server::spawn(config).await.expect("TLS relay should start");
    let https_addr = server.https_addr().expect("HTTPS listener");
    let http_addr = server.http_addr().expect("captive portal listener");

    assert!(https_addr.ip().is_loopback());
    assert!(http_addr.ip().is_loopback());
    assert_ne!(https_addr.port(), http_addr.port());

    let mut roots = RootCertStore::empty();
    for certificate in certificates {
        roots.add(certificate).expect("valid test certificate");
    }
    let client =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("ring supports safe protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth();

    let tcp = TcpStream::connect(https_addr)
        .await
        .expect("connect HTTPS listener");
    let domain = ServerName::try_from("localhost").expect("valid DNS name");
    let mut tls = TlsConnector::from(Arc::new(client))
        .connect(domain, tcp)
        .await
        .expect("self-signed certificate is trusted explicitly");
    tls.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("write health request");

    let mut response = Vec::new();
    tls.read_to_end(&mut response)
        .await
        .expect("read health response");
    let response = String::from_utf8_lossy(&response);
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "unexpected response: {response}"
    );

    server.shutdown().await.expect("clean relay shutdown");
}

#[tokio::test]
async fn pab_iroh_endpoint_uses_only_the_configured_tls_relay() {
    let endpoint_secret = SecretKey::from_bytes(&[31; 32]);
    let endpoint_id = endpoint_secret.public();
    let access = Arc::new(EndpointAllowlist {
        allowed: HashSet::from([endpoint_id]),
        seen: Mutex::new(Vec::new()),
    });
    let (certificates, server_tls) = testing::self_signed_tls_certs_and_config();
    let mut relay = RelayConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.tls = Some(TlsConfig::new(
        (Ipv4Addr::LOCALHOST, 0),
        CertConfig::Manual {
            server_config: server_tls,
        },
    ));
    relay.access = access.clone();
    let mut server_config = ServerConfig::default();
    server_config.relay = Some(relay);
    let server = Server::spawn(server_config)
        .await
        .expect("TLS relay should start");
    let relay_url: RelayUrl = format!(
        "https://localhost:{}",
        server.https_addr().expect("HTTPS listener").port()
    )
    .parse()
    .expect("valid relay URL");
    let endpoint_config = PabEndpointConfig::new(vec![relay_url.clone()])
        .unwrap()
        .with_extra_ca_certificates(certificates);

    let endpoint = PabEndpoint::bind(endpoint_config, endpoint_secret)
        .await
        .expect("PAB endpoint binds");
    let address = endpoint
        .wait_online(Duration::from_secs(10))
        .await
        .expect("PAB endpoint reaches its configured relay");

    assert_eq!(endpoint.id(), endpoint_id);
    assert_eq!(
        address.relay_urls().cloned().collect::<Vec<_>>(),
        vec![relay_url]
    );
    assert!(
        access
            .seen
            .lock()
            .expect("seen lock")
            .contains(&endpoint_id)
    );

    endpoint.close().await;
    server.shutdown().await.expect("clean relay shutdown");
}

#[tokio::test]
async fn endpoint_admission_and_source_identity_survive_tls_forwarding() {
    let a_secret = SecretKey::from_bytes(&[1; 32]);
    let b_secret = SecretKey::from_bytes(&[2; 32]);
    let denied_secret = SecretKey::from_bytes(&[3; 32]);
    let a_id = a_secret.public();
    let b_id = b_secret.public();
    let denied_id = denied_secret.public();
    let access = Arc::new(EndpointAllowlist {
        allowed: HashSet::from([a_id, b_id]),
        seen: Mutex::new(Vec::new()),
    });

    let (certificates, server_tls) = testing::self_signed_tls_certs_and_config();
    let mut relay = RelayConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.tls = Some(TlsConfig::new(
        (Ipv4Addr::LOCALHOST, 0),
        CertConfig::Manual {
            server_config: server_tls,
        },
    ));
    relay.access = access.clone();
    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    let server = Server::spawn(config).await.expect("TLS relay should start");
    let https_addr = server.https_addr().expect("HTTPS listener");
    let relay_url: RelayUrl = format!("https://localhost:{}", https_addr.port())
        .parse()
        .expect("valid relay URL");
    let client_tls = CaTlsConfig::custom_roots(certificates)
        .client_config(default_provider())
        .expect("custom test root");

    let denied = ClientBuilder::new(relay_url.clone(), denied_secret, DnsResolver::new())
        .tls_client_config(client_tls.clone())
        .connect()
        .await;
    assert!(denied.is_err(), "unregistered endpoint must be denied");

    let mut client_a = ClientBuilder::new(relay_url.clone(), a_secret, DnsResolver::new())
        .tls_client_config(client_tls.clone())
        .connect()
        .await
        .expect("endpoint A admitted");
    let mut client_b = ClientBuilder::new(relay_url, b_secret, DnsResolver::new())
        .tls_client_config(client_tls)
        .connect()
        .await
        .expect("endpoint B admitted");

    let payload = Datagrams::from("pab relay identity probe");
    client_a
        .send(ClientToRelayMsg::Datagrams {
            dst_endpoint_id: b_id,
            datagrams: payload.clone(),
        })
        .await
        .expect("send relay datagram");
    let received = tokio::time::timeout(Duration::from_secs(5), client_b.next())
        .await
        .expect("relay receive timeout")
        .expect("relay stream ended")
        .expect("relay receive error");
    match received {
        RelayToClientMsg::Datagrams {
            remote_endpoint_id,
            datagrams,
        } => {
            assert_eq!(remote_endpoint_id, a_id);
            assert_eq!(datagrams, payload);
        }
        other => panic!("unexpected relay message: {other:?}"),
    }

    let seen = access.seen.lock().expect("seen lock").clone();
    assert!(seen.contains(&a_id));
    assert!(seen.contains(&b_id));
    assert!(seen.contains(&denied_id));
    drop(client_a);
    drop(client_b);
    server.shutdown().await.expect("clean relay shutdown");
}

#[tokio::test]
async fn forwarding_control_delays_with_both_endpoint_ids_visible() {
    let a_secret = SecretKey::from_bytes(&[4; 32]);
    let b_secret = SecretKey::from_bytes(&[5; 32]);
    let a_id = a_secret.public();
    let b_id = b_secret.public();
    let forwarding = Arc::new(DelayFirstDatagram::default());

    let (certificates, server_tls) = testing::self_signed_tls_certs_and_config();
    let mut relay = RelayConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.tls = Some(TlsConfig::new(
        (Ipv4Addr::LOCALHOST, 0),
        CertConfig::Manual {
            server_config: server_tls,
        },
    ));
    relay.forwarding = forwarding.clone();
    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    let server = Server::spawn(config).await.expect("TLS relay should start");
    let relay_url: RelayUrl = format!(
        "https://localhost:{}",
        server.https_addr().expect("HTTPS listener").port()
    )
    .parse()
    .expect("valid relay URL");
    let client_tls = CaTlsConfig::custom_roots(certificates)
        .client_config(default_provider())
        .expect("custom test root");
    let mut client_a = ClientBuilder::new(relay_url.clone(), a_secret, DnsResolver::new())
        .tls_client_config(client_tls.clone())
        .connect()
        .await
        .expect("endpoint A connected");
    let mut client_b = ClientBuilder::new(relay_url, b_secret, DnsResolver::new())
        .tls_client_config(client_tls)
        .connect()
        .await
        .expect("endpoint B connected");

    let payload = Datagrams::from("delayed by forwarding control");
    let started = Instant::now();
    client_a
        .send(ClientToRelayMsg::Datagrams {
            dst_endpoint_id: b_id,
            datagrams: payload.clone(),
        })
        .await
        .expect("send relay datagram");
    let received = tokio::time::timeout(Duration::from_secs(2), client_b.next())
        .await
        .expect("relay receive timeout")
        .expect("relay stream ended")
        .expect("relay receive error");
    assert!(started.elapsed() >= Duration::from_millis(50));
    assert!(matches!(
        received,
        RelayToClientMsg::Datagrams { remote_endpoint_id, datagrams }
            if remote_endpoint_id == a_id && datagrams == payload
    ));
    {
        let observations = forwarding.endpoints.lock().expect("endpoint lock");
        assert!(observations.len() >= 2);
        assert!(observations.iter().all(|(src, dst, bytes)| *src == a_id
            && *dst == b_id
            && *bytes == payload.contents.len()));
    }
    drop(client_a);
    drop(client_b);
    server.shutdown().await.expect("clean relay shutdown");
}
