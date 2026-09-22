use std::{sync::Arc, time::Duration};

use axum_server::tls_rustls::RustlsConfig;
use futures_util::{SinkExt, StreamExt};
use iroh_base::SecretKey;
use pab_agent_core::{
    AuthenticatedControlConnection, ControlConnectionPhase, EndpointControlConfig,
    EndpointControlSupervisor, ReconnectPolicy, tls_connector,
};
use pab_protocol::{
    ControlClientMessage, ControlErrorCode, ControlServerMessage, DeploymentId,
    EndpointAuthenticationResult, EndpointKey, EndpointProofPrincipal, EndpointProofResponse,
    EndpointRegistration, EndpointRegistrationResult, EndpointSignature, RelayLimitDefaults,
    RequestId,
};
use pab_relay::{
    PolicySync, RelayControlClient, RelayPolicyRuntime, RelayPolicyState, RelayServiceConfig,
    start_relay_service,
};
use pab_server::{
    ControlApiConfig, ControlApiState, ControlPlane, PasswordPolicy, PostgresStore,
    RelayControlAuth, control_router,
};
use rustls::{ClientConfig, RootCertStore};
use sqlx::PgPool;
use tokio_tungstenite::{
    Connector, connect_async, connect_async_tls_with_config, tungstenite::Message,
};

const RELAY_CONTROL_SECRET: &str = "test-only Relay control secret 0123456789";

async fn send(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    message: &ControlClientMessage,
) {
    let encoded = serde_json::to_string(message).unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        socket.send(Message::Text(encoded.into())),
    )
    .await
    .expect("sending a WSS control message timed out")
    .unwrap();
}

async fn receive(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> ControlServerMessage {
    let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("receiving a WSS control message timed out")
        .unwrap()
        .unwrap();
    let Message::Text(text) = message else {
        panic!("expected a text control message");
    };
    serde_json::from_str(text.as_str()).unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn tls_wss_account_endpoint_and_relay_policy_flow(pool: PgPool) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let store = PostgresStore::from_pool(pool);
    let control = ControlPlane::new(store, PasswordPolicy::default()).unwrap();
    let deployment_id = control
        .initialize_deployment(
            DeploymentId::new(),
            RelayLimitDefaults {
                team_mbps: 20,
                member_mbps: 4,
                personal_mbps: 5,
            },
        )
        .await
        .unwrap();
    let state = ControlApiState::new(
        control,
        deployment_id,
        ControlApiConfig {
            registration_enabled: true,
            ..ControlApiConfig::default()
        },
        RelayControlAuth::new(RELAY_CONTROL_SECRET).unwrap(),
    );

    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let certificate_directory = tempfile::tempdir().unwrap();
    let certificate_path = certificate_directory.path().join("relay-cert.pem");
    let private_key_path = certificate_directory.path().join("relay-key.pem");
    std::fs::write(&certificate_path, certified.cert.pem()).unwrap();
    std::fs::write(&private_key_path, certified.signing_key.serialize_pem()).unwrap();
    let certificate = certified.cert.der().clone();
    let tls = RustlsConfig::from_der(
        vec![certificate.to_vec()],
        certified.signing_key.serialize_der(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = axum_server::Handle::new();
    let server_handle = handle.clone();
    let server = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .unwrap()
            .handle(server_handle)
            .serve(control_router(state).into_make_service())
            .await
            .unwrap();
    });

    let plain_result = tokio::time::timeout(
        Duration::from_secs(2),
        connect_async(format!("ws://127.0.0.1:{}/control", address.port())),
    )
    .await;
    assert!(
        !matches!(plain_result, Ok(Ok(_))),
        "the TLS listener must not accept plaintext WebSocket connections"
    );

    let mut roots = RootCertStore::empty();
    roots.add(certificate).unwrap();
    let client_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = Connector::Rustls(Arc::new(client_config));
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(5),
        connect_async_tls_with_config(
            format!("wss://localhost:{}/control", address.port()),
            None,
            false,
            Some(connector.clone()),
        ),
    )
    .await
    .expect("WSS connection timed out")
    .unwrap();

    let register_request_id = RequestId::new();
    send(
        &mut socket,
        &ControlClientMessage::RegisterAccount {
            request_id: register_request_id,
            username: "Alice".to_owned(),
            password: "correct horse battery staple".to_owned(),
        },
    )
    .await;
    let (user_id, personal_tenant_id) = match receive(&mut socket).await {
        ControlServerMessage::AccountAuthenticated {
            request_id,
            user_id,
            personal_tenant_id,
            ..
        } => {
            assert_eq!(request_id, register_request_id);
            (user_id, personal_tenant_id)
        }
        response => panic!("unexpected register response: {response:?}"),
    };

    let secret = SecretKey::generate();
    let begin_request_id = RequestId::new();
    send(
        &mut socket,
        &ControlClientMessage::BeginEndpointRegistration {
            request_id: begin_request_id,
            tenant_id: personal_tenant_id,
            endpoint_key: EndpointKey::new(*secret.public().as_bytes()),
            registration: EndpointRegistration::User,
        },
    )
    .await;
    let challenge = match receive(&mut socket).await {
        ControlServerMessage::EndpointChallenge {
            request_id,
            challenge,
        } => {
            assert_eq!(request_id, begin_request_id);
            assert_eq!(
                challenge.principal,
                EndpointProofPrincipal::User { user_id }
            );
            assert!(!challenge.connection_id.as_uuid().is_nil());
            challenge
        }
        response => panic!("unexpected challenge response: {response:?}"),
    };

    let complete_request_id = RequestId::new();
    send(
        &mut socket,
        &ControlClientMessage::CompleteEndpointRegistration {
            request_id: complete_request_id,
            proof: EndpointProofResponse {
                challenge_id: challenge.challenge_id,
                signature: EndpointSignature::from_bytes(
                    secret.sign(&challenge.signing_message()).to_bytes(),
                ),
            },
        },
    )
    .await;
    match receive(&mut socket).await {
        ControlServerMessage::EndpointRegistered { request_id, result } => {
            assert_eq!(request_id, complete_request_id);
            assert_eq!(
                result,
                EndpointRegistrationResult::User {
                    tenant_id: personal_tenant_id,
                    endpoint_key: EndpointKey::new(*secret.public().as_bytes()),
                }
            );
        }
        response => panic!("unexpected registration response: {response:?}"),
    }

    drop(socket);

    let (mut endpoint_socket, _) = tokio::time::timeout(
        Duration::from_secs(5),
        connect_async_tls_with_config(
            format!("wss://localhost:{}/control", address.port()),
            None,
            false,
            Some(connector.clone()),
        ),
    )
    .await
    .expect("endpoint WSS connection timed out")
    .unwrap();
    let authenticate_request_id = RequestId::new();
    send(
        &mut endpoint_socket,
        &ControlClientMessage::BeginEndpointAuthentication {
            request_id: authenticate_request_id,
            endpoint_key: EndpointKey::new(*secret.public().as_bytes()),
        },
    )
    .await;
    let authentication_challenge = match receive(&mut endpoint_socket).await {
        ControlServerMessage::EndpointChallenge {
            request_id,
            challenge,
        } => {
            assert_eq!(request_id, authenticate_request_id);
            assert_eq!(
                challenge.principal,
                EndpointProofPrincipal::User { user_id }
            );
            challenge
        }
        response => panic!("unexpected endpoint challenge response: {response:?}"),
    };
    let conflicting_login_request_id = RequestId::new();
    send(
        &mut endpoint_socket,
        &ControlClientMessage::Login {
            request_id: conflicting_login_request_id,
            username: "alice".to_owned(),
            password: "correct horse battery staple".to_owned(),
        },
    )
    .await;
    assert!(matches!(
        receive(&mut endpoint_socket).await,
        ControlServerMessage::Error {
            request_id: Some(request_id),
            code: ControlErrorCode::InvalidState,
            ..
        } if request_id == conflicting_login_request_id
    ));
    let complete_authentication_request_id = RequestId::new();
    send(
        &mut endpoint_socket,
        &ControlClientMessage::CompleteEndpointAuthentication {
            request_id: complete_authentication_request_id,
            proof: EndpointProofResponse {
                challenge_id: authentication_challenge.challenge_id,
                signature: EndpointSignature::from_bytes(
                    secret
                        .sign(&authentication_challenge.signing_message())
                        .to_bytes(),
                ),
            },
        },
    )
    .await;
    assert_eq!(
        receive(&mut endpoint_socket).await,
        ControlServerMessage::EndpointAuthenticated {
            request_id: complete_authentication_request_id,
            result: EndpointAuthenticationResult {
                tenant_id: personal_tenant_id,
                endpoint_key: EndpointKey::new(*secret.public().as_bytes()),
                principal: EndpointProofPrincipal::User { user_id },
            },
        }
    );
    drop(endpoint_socket);

    let connector = tls_connector(Some(&std::fs::read(&certificate_path).unwrap())).unwrap();
    let agent_config = EndpointControlConfig {
        url: format!("wss://localhost:{}/control", address.port()),
        deployment_id,
        tenant_id: personal_tenant_id,
        principal: EndpointProofPrincipal::User { user_id },
        operation_timeout: Duration::from_secs(5),
    };
    let agent_connection =
        AuthenticatedControlConnection::connect(&agent_config, &secret, connector.clone())
            .await
            .unwrap();
    assert_eq!(
        agent_connection.identity(),
        EndpointAuthenticationResult {
            tenant_id: personal_tenant_id,
            endpoint_key: EndpointKey::new(*secret.public().as_bytes()),
            principal: EndpointProofPrincipal::User { user_id },
        }
    );
    let mut agent_connection = agent_connection;
    agent_connection
        .heartbeat(b"pab-test-heartbeat".to_vec(), Duration::from_secs(5))
        .await
        .unwrap();
    agent_connection.close().await.unwrap();

    let supervisor = EndpointControlSupervisor::new(
        agent_config,
        secret.clone(),
        connector.clone(),
        ReconnectPolicy {
            heartbeat_interval: Duration::from_secs(1),
            heartbeat_timeout: Duration::from_millis(500),
            initial_delay: Duration::from_millis(50),
            max_delay: Duration::from_millis(200),
        },
    )
    .unwrap();
    let supervisor = supervisor.spawn();
    let mut connection_status = supervisor.status();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if connection_status.borrow().phase == ControlConnectionPhase::Authenticated {
                break;
            }
            connection_status.changed().await.unwrap();
        }
    })
    .await
    .expect("endpoint supervisor did not authenticate");
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    assert_eq!(
        connection_status.borrow().phase,
        ControlConnectionPhase::Authenticated
    );
    supervisor.shutdown().await.unwrap();
    assert_eq!(
        connection_status.borrow().phase,
        ControlConnectionPhase::Stopped
    );

    let (mut login_socket, _) = tokio::time::timeout(
        Duration::from_secs(5),
        connect_async_tls_with_config(
            format!("wss://localhost:{}/control", address.port()),
            None,
            false,
            Some(connector.clone()),
        ),
    )
    .await
    .expect("second WSS connection timed out")
    .unwrap();
    let login_request_id = RequestId::new();
    send(
        &mut login_socket,
        &ControlClientMessage::Login {
            request_id: login_request_id,
            username: "alice".to_owned(),
            password: "correct horse battery staple".to_owned(),
        },
    )
    .await;
    match receive(&mut login_socket).await {
        ControlServerMessage::AccountAuthenticated {
            request_id,
            user_id: logged_in_user,
            personal_tenant_id: logged_in_tenant,
            ..
        } => {
            assert_eq!(request_id, login_request_id);
            assert_eq!(logged_in_user, user_id);
            assert_eq!(logged_in_tenant, personal_tenant_id);
        }
        response => panic!("unexpected login response: {response:?}"),
    }
    drop(login_socket);

    let relay_url = format!("wss://localhost:{}/relay-control", address.port());
    let unauthorized = tokio::time::timeout(
        Duration::from_secs(5),
        connect_async_tls_with_config(relay_url.clone(), None, false, Some(connector.clone())),
    )
    .await
    .expect("unauthorized Relay WSS connection timed out");
    assert!(
        unauthorized.is_err(),
        "Relay control must reject a connection without its internal secret"
    );

    let mut relay_client =
        RelayControlClient::connect(&relay_url, deployment_id, RELAY_CONTROL_SECRET, connector)
            .await
            .unwrap();
    let relay_policy = RelayPolicyRuntime::new(
        RelayPolicyState::new(deployment_id, Duration::from_millis(100)).unwrap(),
    );
    let first_sync = relay_client.sync_policy(&relay_policy).await.unwrap();
    let policy_version = match first_sync {
        PolicySync::Updated { policy_version } => policy_version,
        other => panic!("unexpected initial policy result: {other:?}"),
    };
    assert_eq!(
        relay_policy
            .endpoint_owner(secret.public())
            .expect("Relay policy lock"),
        Some(pab_protocol::RelayEndpointOwner::User {
            scope: pab_protocol::TrafficScope::Personal {
                tenant_id: personal_tenant_id,
                user_id,
            },
        })
    );
    assert_eq!(
        relay_client.sync_policy(&relay_policy).await.unwrap(),
        PolicySync::Unchanged { policy_version }
    );

    let running_relay = tokio::time::timeout(
        Duration::from_secs(10),
        start_relay_service(RelayServiceConfig {
            deployment_id,
            control_url: format!("wss://localhost:{}/relay-control", address.port()),
            control_secret: RELAY_CONTROL_SECRET.to_owned(),
            control_ca_cert: Some(certificate_path.clone()),
            tls_cert: certificate_path,
            tls_key: private_key_path,
            https_bind: "127.0.0.1:0".parse().unwrap(),
            captive_bind: "127.0.0.1:0".parse().unwrap(),
            quic_bind: "127.0.0.1:0".parse().unwrap(),
            policy_refresh_interval: Duration::from_secs(20),
            reconnect_initial_delay: Duration::from_millis(50),
            reconnect_max_delay: Duration::from_secs(1),
            limiter_burst: Duration::from_millis(100),
        }),
    )
    .await
    .expect("production Relay startup timed out")
    .unwrap();
    assert!(running_relay.https_addr().unwrap().ip().is_loopback());
    assert!(running_relay.quic_addr().unwrap().ip().is_loopback());
    assert!(running_relay.captive_addr().unwrap().ip().is_loopback());
    tokio::time::timeout(Duration::from_secs(5), running_relay.shutdown())
        .await
        .expect("production Relay shutdown timed out")
        .unwrap();

    handle.graceful_shutdown(Some(Duration::from_secs(1)));
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("TLS server shutdown timed out")
        .unwrap();
}
