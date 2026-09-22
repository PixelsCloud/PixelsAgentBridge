use std::{sync::Arc, time::Duration};

use axum_server::tls_rustls::RustlsConfig;
use futures_util::{SinkExt, StreamExt};
use iroh_base::SecretKey;
use pab_protocol::{
    ControlClientMessage, ControlServerMessage, DeploymentId, EndpointKey, EndpointProofResponse,
    EndpointRegistration, EndpointRegistrationResult, EndpointSignature, RelayLimitDefaults,
    RequestId,
};
use pab_server::{
    ControlApiConfig, ControlApiState, ControlPlane, PasswordPolicy, PostgresStore, control_router,
};
use rustls::{ClientConfig, RootCertStore};
use sqlx::PgPool;
use tokio_tungstenite::{
    Connector, connect_async, connect_async_tls_with_config, tungstenite::Message,
};

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
async fn tls_wss_registration_login_and_endpoint_proof(pool: PgPool) {
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
        },
    );

    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
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
            assert_eq!(challenge.user_id, user_id);
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

    let (mut login_socket, _) = tokio::time::timeout(
        Duration::from_secs(5),
        connect_async_tls_with_config(
            format!("wss://localhost:{}/control", address.port()),
            None,
            false,
            Some(connector),
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

    handle.graceful_shutdown(Some(Duration::from_secs(1)));
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("TLS server shutdown timed out")
        .unwrap();
}
