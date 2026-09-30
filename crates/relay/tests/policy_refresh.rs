//! Use the production 20-second policy interval and real TLS Relay/QUIC traffic.
//! Direct transport is disabled so it cannot hide a stale connection grant.
#![cfg(debug_assertions)]

use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures_util::{SinkExt, StreamExt};
use iroh_base::SecretKey;
use pab_protocol::{
    DeploymentId, DeviceId, EndpointKey, RELAY_POLICY_SCHEMA_VERSION, RelayConnectionIntent,
    RelayControlClientMessage, RelayControlServerMessage, RelayEndpointOwner, RelayEndpointPolicy,
    RelayLimitDefaults, RelayPolicySnapshot, TenantId,
};
use pab_relay::{RelayServiceConfig, start_relay_service};
use pab_transport::{ConnectionPath, PabEndpoint, PabEndpointConfig};
use tokio::{net::TcpListener, sync::watch};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        Message,
        handshake::server::{ErrorResponse, Request, Response},
    },
};

const CONTROL_SECRET: &str = "test-only policy refresh control secret 0123456789";

// Tungstenite requires this unboxed HTTP error response in its callback API.
#[allow(clippy::result_large_err)]
fn authenticate_control(request: &Request, response: Response) -> Result<Response, ErrorResponse> {
    assert_eq!(
        request
            .headers()
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap(),
        format!("Bearer {CONTROL_SECRET}")
    );
    Ok(response)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[tokio::test]
async fn first_relay_connection_refreshes_a_new_grant_before_timeout() {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let cert_path = directory.path().join("cert.pem");
    let key_path = directory.path().join("key.pem");
    std::fs::write(&cert_path, certified.cert.pem()).unwrap();
    std::fs::write(&key_path, certified.signing_key.serialize_pem()).unwrap();
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![certified.cert.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der()).into(),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let control_url = format!(
        "wss://localhost:{}/relay-control",
        listener.local_addr().unwrap().port()
    );
    let deployment_id = DeploymentId::new();
    let tenant_id = TenantId::new();
    let device_id = DeviceId::new();
    let operator = SecretKey::generate();
    let device = SecretKey::generate();
    let new_operator = SecretKey::generate();
    let policy = Arc::new(Mutex::new(RelayPolicySnapshot {
        schema_version: RELAY_POLICY_SCHEMA_VERSION,
        deployment_id,
        policy_version: 1,
        issued_at_unix_ms: now_ms() - 1,
        expires_at_unix_ms: now_ms() + 60_000,
        defaults: RelayLimitDefaults {
            team_mbps: 20,
            member_mbps: 4,
            personal_mbps: 5,
        },
        team_limits: vec![],
        endpoints: vec![
            RelayEndpointPolicy {
                endpoint_key: EndpointKey::new(*operator.public().as_bytes()),
                owner: RelayEndpointOwner::Guest,
            },
            RelayEndpointPolicy {
                endpoint_key: EndpointKey::new(*device.public().as_bytes()),
                owner: RelayEndpointOwner::Device {
                    tenant_id,
                    device_id,
                },
            },
        ],
        connection_intents: vec![],
    }));
    let server_policy = policy.clone();
    let (applied, mut version) = watch::channel(0);
    let control_server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let socket = TlsAcceptor::from(Arc::new(tls))
            .accept(socket)
            .await
            .unwrap();
        let mut socket = accept_hdr_async(socket, authenticate_control)
            .await
            .unwrap();
        while let Some(Ok(Message::Text(text))) = socket.next().await {
            let RelayControlClientMessage::GetPolicy {
                request_id,
                known_policy_version,
                ..
            } = serde_json::from_str(text.as_str()).unwrap();
            let mut snapshot = server_policy.lock().unwrap().clone();
            snapshot.issued_at_unix_ms = now_ms() - 1;
            snapshot.expires_at_unix_ms = now_ms() + 60_000;
            let policy_version = snapshot.policy_version;
            let response = if known_policy_version == Some(policy_version) {
                RelayControlServerMessage::PolicyUnchanged {
                    request_id,
                    deployment_id,
                    policy_version,
                    expires_at_unix_ms: snapshot.expires_at_unix_ms,
                }
            } else {
                RelayControlServerMessage::PolicySnapshot {
                    request_id,
                    snapshot,
                }
            };
            socket
                .send(Message::Text(
                    serde_json::to_string(&response).unwrap().into(),
                ))
                .await
                .unwrap();
            applied.send_replace(policy_version);
        }
    });
    let relay = start_relay_service(RelayServiceConfig {
        deployment_id,
        control_url,
        control_secret: CONTROL_SECRET.to_owned(),
        control_ca_cert: Some(cert_path.clone()),
        tls_cert: cert_path,
        tls_key: key_path,
        https_bind: "127.0.0.1:0".parse().unwrap(),
        captive_bind: "127.0.0.1:0".parse().unwrap(),
        quic_bind: "127.0.0.1:0".parse().unwrap(),
        policy_refresh_interval: Duration::from_secs(20),
        reconnect_interval: Duration::from_millis(50),
        limiter_burst: Duration::from_millis(100),
    })
    .await
    .unwrap();
    let relay_url = format!("https://localhost:{}", relay.https_addr().unwrap().port())
        .parse()
        .unwrap();
    let config = PabEndpointConfig::new(vec![relay_url])
        .unwrap()
        .with_extra_ca_certificates(vec![certified.cert.der().clone()])
        .relay_only_for_testing();
    let client = PabEndpoint::bind(config.clone(), operator.clone())
        .await
        .unwrap();
    let peer = PabEndpoint::bind(config.clone(), device).await.unwrap();
    client.wait_online(Duration::from_secs(5)).await.unwrap();
    peer.wait_online(Duration::from_secs(5)).await.unwrap();
    assert_eq!(*version.borrow_and_update(), 1);
    assert!(peer.address().ip_addrs().next().is_none());

    // Simulate the address lookup committing its new pair grant after Relay's
    // initial snapshot. The next periodic refresh is still almost 20 s away.
    {
        let mut snapshot = policy.lock().unwrap();
        snapshot.policy_version = 2;
        snapshot.connection_intents.push(RelayConnectionIntent {
            operator_endpoint_key: EndpointKey::new(*operator.public().as_bytes()),
            device_id,
            expires_at_unix_ms: now_ms() + 60_000,
        });
    }
    let address = pab_transport::PabEndpointAddress {
        relay_urls: peer
            .address()
            .relay_urls()
            .map(ToString::to_string)
            .collect(),
        direct_addresses: vec![],
    };
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            client.connect(*peer.id().as_bytes(), &address, Duration::from_secs(5)),
            peer.accept()
        )
    })
    .await;
    let (connection, accepted) = match result {
        Ok((Ok(connection), Ok(Some(accepted)))) => (connection, accepted),
        other => {
            client.close().await;
            peer.close().await;
            relay.shutdown().await.unwrap();
            control_server.abort();
            panic!(
                "first Relay connection timed out on a stale policy: {}",
                if other.is_err() {
                    "timeout"
                } else {
                    "handshake failed"
                }
            );
        }
    };
    assert_eq!(connection.selected_path(), Some(ConnectionPath::Relay));
    let mut stream = connection.open_bi(Duration::from_secs(3)).await.unwrap();
    stream
        .send_json(&"policy-refresh-ok", Duration::from_secs(3))
        .await
        .unwrap();
    let mut received = accepted.accept_bi(Duration::from_secs(3)).await.unwrap();
    assert_eq!(
        received
            .receive_json::<String>(Duration::from_secs(3))
            .await
            .unwrap(),
        "policy-refresh-ok"
    );
    println!(
        "first Relay connection succeeded in {:?}",
        started.elapsed()
    );
    assert_eq!(*version.borrow_and_update(), 2);

    // Endpoint admission must also refresh a newly registered key instead of
    // waiting for the next periodic snapshot and triggering client backoff.
    {
        let mut snapshot = policy.lock().unwrap();
        snapshot.policy_version = 3;
        snapshot.endpoints.push(RelayEndpointPolicy {
            endpoint_key: EndpointKey::new(*new_operator.public().as_bytes()),
            owner: RelayEndpointOwner::Guest,
        });
    }
    let fresh = PabEndpoint::bind(config, new_operator).await.unwrap();
    fresh
        .wait_online(Duration::from_secs(5))
        .await
        .expect("new endpoint admission waited for periodic policy refresh");
    assert_eq!(*version.borrow_and_update(), 3);
    fresh.close().await;
    connection.close(b"test complete");
    accepted.close(b"test complete");
    client.close().await;
    peer.close().await;
    relay.shutdown().await.unwrap();
    control_server.abort();
    let _ = control_server.await;
}
