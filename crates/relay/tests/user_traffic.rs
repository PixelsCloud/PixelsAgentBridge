//! Real TLS Relay and two live QUIC connections; no direct transport fallback.
#![cfg(debug_assertions)]
use iroh_base::SecretKey;
use iroh_relay::server::{CertConfig, RelayConfig, Server, ServerConfig, TlsConfig};
use pab_protocol::*;
use pab_relay::{RelayPolicyRuntime, RelayPolicyState};
use pab_transport::{ConnectionPath, PabConnection, PabEndpoint, PabEndpointConfig};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

async fn pump(sender: PabConnection, receiver: PabConnection, total: Arc<AtomicU64>, marker: u8) {
    let send = async {
        let mut stream = sender.open_bi(Duration::from_secs(10)).await.unwrap();
        let data = vec![marker; 32 * 1024];
        loop {
            stream
                .send_binary_frame(&data, Duration::from_secs(10))
                .await
                .unwrap();
        }
    };
    let receive = async {
        let mut stream = receiver.accept_bi(Duration::from_secs(10)).await.unwrap();
        loop {
            let data = stream
                .receive_binary_frame(Duration::from_secs(10))
                .await
                .unwrap();
            assert!(
                data.iter().all(|b| *b == marker),
                "binary data changed during account switch"
            );
            total.fetch_add(data.len() as u64, Ordering::Relaxed);
        }
    };
    tokio::select! { _ = send => {}, _ = receive => {} }
}

async fn rate(label: &str, total: &AtomicU64) -> f64 {
    tokio::time::sleep(Duration::from_secs(1)).await;
    let before = total.load(Ordering::Relaxed);
    let start = std::time::Instant::now();
    tokio::time::sleep(Duration::from_secs(3)).await;
    let mbps = (total.load(Ordering::Relaxed) - before) as f64 * 8.0
        / start.elapsed().as_secs_f64()
        / 1_000_000.0;
    println!("{label}: {mbps:.3} Mbps received across two existing Relay connections");
    mbps
}

#[tokio::test]
async fn live_account_switch_and_user_limit_share_budget_without_reconnecting() {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.cert.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
    )
    .unwrap();
    let runtime =
        RelayPolicyRuntime::new(RelayPolicyState::new(Duration::from_millis(100)).unwrap());
    let tenant_id = TenantId::new();
    let device_id = DeviceId::new();
    let user_id = UserId::new();
    let peer_secret = SecretKey::generate();
    let keys = [SecretKey::generate(), SecretKey::generate()];
    let mut policy = RelayPolicySnapshot {
        schema_version: RELAY_POLICY_SCHEMA_VERSION,
        policy_version: 1,
        issued_at_unix_ms: now_ms() - 1,
        expires_at_unix_ms: now_ms() + 120_000,
        defaults: RelayLimitDefaults {
            user_mbps: 4,
            guest_mbps: 1,
        },
        user_limits: vec![],
        endpoints: vec![RelayEndpointPolicy {
            endpoint_key: EndpointKey::new(*peer_secret.public().as_bytes()),
            owner: RelayEndpointOwner::Device {
                tenant_id,
                device_id,
            },
        }],
        connection_intents: vec![],
    };
    for key in &keys {
        let endpoint_key = EndpointKey::new(*key.public().as_bytes());
        policy.endpoints.push(RelayEndpointPolicy {
            endpoint_key,
            owner: RelayEndpointOwner::Guest,
        });
        policy.connection_intents.push(RelayConnectionIntent {
            operator_endpoint_key: endpoint_key,
            device_id,
            expires_at_unix_ms: now_ms() + 120_000,
        });
    }
    runtime.apply_snapshot(policy.clone()).unwrap();
    let mut config = RelayConfig::new("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap());
    config.tls = Some(TlsConfig::new(
        "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap(),
        CertConfig::Manual { server_config: tls },
    ));
    config.access = Arc::new(runtime.clone());
    config.forwarding = Arc::new(runtime.clone());
    let mut server_config = ServerConfig::default();
    server_config.relay = Some(config);
    let relay = Server::spawn(server_config).await.unwrap();
    let endpoint_config = PabEndpointConfig::new(vec![
        format!("https://localhost:{}", relay.https_addr().unwrap().port())
            .parse()
            .unwrap(),
    ])
    .unwrap()
    .with_extra_ca_certificates(vec![cert.cert.der().clone()])
    .relay_only_for_testing();
    let peer = PabEndpoint::bind(endpoint_config.clone(), peer_secret)
        .await
        .unwrap();
    let first = PabEndpoint::bind(endpoint_config.clone(), keys[0].clone())
        .await
        .unwrap();
    let second = PabEndpoint::bind(endpoint_config, keys[1].clone())
        .await
        .unwrap();
    peer.wait_online(Duration::from_secs(10)).await.unwrap();
    first.wait_online(Duration::from_secs(10)).await.unwrap();
    second.wait_online(Duration::from_secs(10)).await.unwrap();
    let address = pab_transport::PabEndpointAddress {
        relay_urls: peer
            .address()
            .relay_urls()
            .map(ToString::to_string)
            .collect(),
        direct_addresses: vec![],
    };
    let (first_conn, peer_first) = tokio::join!(
        first.connect(*peer.id().as_bytes(), &address, Duration::from_secs(10)),
        peer.accept()
    );
    let (second_conn, peer_second) = tokio::join!(
        second.connect(*peer.id().as_bytes(), &address, Duration::from_secs(10)),
        peer.accept()
    );
    let first_conn = first_conn.unwrap();
    let peer_first = peer_first.unwrap().unwrap();
    let second_conn = second_conn.unwrap();
    let peer_second = peer_second.unwrap().unwrap();
    assert_eq!(first_conn.selected_path(), Some(ConnectionPath::Relay));
    assert_eq!(second_conn.selected_path(), Some(ConnectionPath::Relay));
    let count = Arc::new(AtomicU64::new(0));
    let workers = [
        tokio::spawn(pump(
            first_conn.clone(),
            peer_first.clone(),
            count.clone(),
            17,
        )),
        tokio::spawn(pump(
            second_conn.clone(),
            peer_second.clone(),
            count.clone(),
            29,
        )),
    ];
    let guest = rate("guest 1 Mbps per endpoint", &count).await;
    policy.policy_version += 1;
    for item in &mut policy.endpoints[1..] {
        item.owner = RelayEndpointOwner::User {
            scope: TrafficScope::User { user_id },
        };
    }
    runtime.apply_snapshot(policy.clone()).unwrap();
    let user = rate("same user 4 Mbps aggregate", &count).await;
    policy.policy_version += 1;
    policy.user_limits = vec![UserRelayLimit { user_id, mbps: 2 }];
    runtime.apply_snapshot(policy.clone()).unwrap();
    let limited = rate("same user override 2 Mbps aggregate", &count).await;
    policy.policy_version += 1;
    for item in &mut policy.endpoints[1..] {
        item.owner = RelayEndpointOwner::Guest;
    }
    runtime.apply_snapshot(policy).unwrap();
    let logged_out = rate("logout to guest", &count).await;
    for worker in workers {
        worker.abort();
        let _ = worker.await;
    }
    assert_eq!(first_conn.selected_path(), Some(ConnectionPath::Relay));
    assert_eq!(second_conn.selected_path(), Some(ConnectionPath::Relay));
    first_conn.close(b"complete");
    second_conn.close(b"complete");
    peer_first.close(b"complete");
    peer_second.close(b"complete");
    first.close().await;
    second.close().await;
    peer.close().await;
    relay.shutdown().await.unwrap();
    assert!((0.4..2.6).contains(&guest), "guest throughput {guest}");
    assert!((1.0..5.2).contains(&user), "user throughput {user}");
    assert!(
        (0.4..2.6).contains(&limited),
        "aggregate limit was multiplied by endpoint count: {limited}"
    );
    assert!(
        (0.4..2.6).contains(&logged_out),
        "logout throughput {logged_out}"
    );
    assert!(
        user > limited * 1.25,
        "live limit change did not affect throughput"
    );
}
