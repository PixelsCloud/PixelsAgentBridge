//! Isolated WSS discovery/proof fixture plus a real TLS iroh Relay. No production state.
use axum::{
    Json,
    extract::{
        State as AppState,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    routing::{get, post, put},
};
use futures_util::StreamExt;
use iroh_base::{PublicKey, Signature};
use pab_protocol::*;
use serde_json::json;
use std::{collections::HashMap, io::Write, sync::Arc, time::Duration};
use tokio::sync::Mutex;
type Socket = WebSocket;
type Error = Box<dyn std::error::Error + Send + Sync>;
#[derive(Default)]
struct State {
    endpoints: HashMap<EndpointKey, EndpointRegistrationResult>,
    network: Option<DeviceNetworkUpdate>,
    contexts: HashMap<EndpointKey, EndpointUserContext>,
    account_password: Option<String>,
    token: String,
    origin: String,
}
fn device() -> DeviceRef {
    DeviceRef {
        tenant_id: TenantId::from_u128(9002),
        device_id: DeviceId::from_u128(9001),
    }
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
fn emit(value: serde_json::Value) {
    println!("{value}");
    let _ = std::io::stdout().flush();
}
async fn receive(socket: &mut Socket) -> Result<ControlClientMessage, Error> {
    loop {
        match tokio::time::timeout(Duration::from_secs(120), socket.next())
            .await?
            .ok_or("closed")??
        {
            Message::Text(text) => return Ok(serde_json::from_str(&text)?),
            Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await?,
            _ => return Err("closed".into()),
        }
    }
}
async fn send(socket: &mut Socket, message: ControlServerMessage) -> Result<(), Error> {
    socket
        .send(Message::Text(serde_json::to_string(&message)?.into()))
        .await?;
    Ok(())
}
async fn handle(mut socket: Socket, state: Arc<Mutex<State>>) -> Result<(), Error> {
    let begin = receive(&mut socket).await?;
    let (request_id, key, register, principal, purpose) = match begin {
        ControlClientMessage::BeginUnclaimedDeviceRegistration {
            request_id,
            endpoint_key,
            ..
        } => (
            request_id,
            endpoint_key,
            true,
            EndpointProofPrincipal::Device {
                device_id: device().device_id,
            },
            EndpointProofPurpose::RegisterUnclaimedDevice,
        ),
        ControlClientMessage::BeginGuestEndpointRegistration {
            request_id,
            endpoint_key,
        } => (
            request_id,
            endpoint_key,
            true,
            EndpointProofPrincipal::Guest,
            EndpointProofPurpose::RegisterGuestEndpoint,
        ),
        ControlClientMessage::BeginEndpointAuthentication {
            request_id,
            endpoint_key,
        } => {
            let lock = state.lock().await;
            let principal = match lock
                .endpoints
                .get(&endpoint_key)
                .ok_or("unknown endpoint")?
            {
                EndpointRegistrationResult::Device { device_id, .. } => {
                    EndpointProofPrincipal::Device {
                        device_id: *device_id,
                    }
                }
                _ => EndpointProofPrincipal::Guest,
            };
            (
                request_id,
                endpoint_key,
                false,
                principal,
                EndpointProofPurpose::AuthenticateRegisteredEndpoint,
            )
        }
        _ => return Err("unexpected request".into()),
    };
    let challenge = EndpointProofChallenge {
        schema_version: ENDPOINT_PROOF_SCHEMA_VERSION,
        challenge_id: ChallengeId::new(),
        connection_id: ConnectionId::new(),
        principal,
        tenant_id: device().tenant_id,
        endpoint_key: key,
        purpose,
        issued_at_unix_ms: now() - 100,
        expires_at_unix_ms: now() + 30000,
        nonce: iroh_base::SecretKey::generate().to_bytes(),
    };
    send(
        &mut socket,
        ControlServerMessage::EndpointChallenge {
            request_id,
            challenge: challenge.clone(),
        },
    )
    .await?;
    let (request_id, proof) = match receive(&mut socket).await? {
        ControlClientMessage::CompleteEndpointRegistration { request_id, proof } if register => {
            (request_id, proof)
        }
        ControlClientMessage::CompleteEndpointAuthentication { request_id, proof } if !register => {
            (request_id, proof)
        }
        _ => return Err("wrong proof".into()),
    };
    if proof.challenge_id != challenge.challenge_id {
        return Err("wrong challenge".into());
    }
    PublicKey::from_bytes(key.as_bytes())?.verify(
        &challenge.signing_message(),
        &Signature::from_bytes(&proof.signature.to_bytes()),
    )?;
    if register {
        let result = match principal {
            EndpointProofPrincipal::Device { device_id } => EndpointRegistrationResult::Device {
                tenant_id: device().tenant_id,
                device_id,
                device_code: DeviceCode::new(987654321)?,
                endpoint_key: key,
            },
            _ => EndpointRegistrationResult::Guest {
                tenant_id: device().tenant_id,
                endpoint_key: key,
            },
        };
        state.lock().await.endpoints.insert(key, result.clone());
        send(
            &mut socket,
            ControlServerMessage::EndpointRegistered { request_id, result },
        )
        .await?;
        return Ok(());
    }
    send(
        &mut socket,
        ControlServerMessage::EndpointAuthenticated {
            request_id,
            result: EndpointAuthenticationResult {
                tenant_id: device().tenant_id,
                endpoint_key: key,
                principal,
            },
        },
    )
    .await?;
    loop {
        let request = receive(&mut socket).await?;
        let request_id = request.request_id();
        let response = match request {
            ControlClientMessage::PublishDeviceHello { hello, .. }
                if matches!(principal, EndpointProofPrincipal::Device { .. }) =>
            {
                if hello.execution_context.os_family != OsFamily::Android {
                    return Err("expected Android".into());
                }
                emit(json!({"event":"android_hello"}));
                ControlServerMessage::DeviceHelloAccepted {
                    request_id,
                    result: DeviceHelloResult {
                        device_ref: device(),
                        environment_revision: hello.execution_context.environment_revision,
                        accepted_at_unix_ms: now(),
                    },
                }
            }
            ControlClientMessage::PublishDeviceNetwork { update, .. }
                if matches!(principal, EndpointProofPrincipal::Device { .. }) =>
            {
                let result = DeviceNetworkResult {
                    device_ref: device(),
                    endpoint_instance_id: update.endpoint_instance_id,
                    address_revision: update.address_revision,
                    accepted_at_unix_ms: now(),
                };
                state.lock().await.network = Some(*update);
                emit(json!({"event":"network_published"}));
                ControlServerMessage::DeviceNetworkAccepted { request_id, result }
            }
            ControlClientMessage::GetRelayDirectory { .. } => {
                ControlServerMessage::RelayDirectory {
                    request_id,
                    directory: None,
                }
            }
            ControlClientMessage::ResolveDeviceCode { device_code, .. }
                if device_code.to_string() == "987654321" =>
            {
                ControlServerMessage::DeviceCodeResolved {
                    request_id,
                    device_ref: device(),
                }
            }
            ControlClientMessage::GetDevicePresence { device_code, .. } => {
                ControlServerMessage::DevicePresenceFound {
                    request_id,
                    presence: DevicePresence {
                        code: device_code,
                        name: "Android fixture".into(),
                        online: state.lock().await.network.is_some(),
                    },
                }
            }
            ControlClientMessage::GetDeviceNetwork { device_ref, .. } if device_ref == device() => {
                let update = state.lock().await.network.clone().ok_or("not ready")?;
                ControlServerMessage::DeviceNetworkFound {
                    request_id,
                    snapshot: Box::new(DeviceNetworkSnapshot {
                        schema_version: DEVICE_NETWORK_SCHEMA_VERSION,
                        device_ref,
                        endpoint_key: update.endpoint_key,
                        endpoint_instance_id: update.endpoint_instance_id,
                        address_revision: update.address_revision,
                        relay_urls: update.relay_urls,
                        direct_addresses: update.direct_addresses,
                        observed_at_unix_ms: update.observed_at_unix_ms,
                        accepted_at_unix_ms: now(),
                    }),
                }
            }
            ControlClientMessage::AuthorizeDevicePeer {
                peer_endpoint_key, ..
            }
            | ControlClientMessage::ReportAuthenticatedDevicePeer {
                peer_endpoint_key, ..
            } if matches!(principal, EndpointProofPrincipal::Device { .. }) => {
                if !matches!(
                    state.lock().await.endpoints.get(&peer_endpoint_key),
                    Some(EndpointRegistrationResult::Guest { .. })
                ) {
                    return Err("unregistered caller".into());
                }
                let context = state
                    .lock()
                    .await
                    .contexts
                    .get(&peer_endpoint_key)
                    .filter(|c| c.user.is_some())
                    .cloned()
                    .ok_or("account login required")?;
                ControlServerMessage::DevicePeerAuthorized {
                    request_id,
                    result: AuthorizedDevicePeer {
                        user_context: context,
                        device_ref: device(),
                        peer_endpoint_key,
                        operator: OperatorRef::guest(peer_endpoint_key),
                        authorized_at_unix_ms: now(),
                    },
                }
            }
            _ => ControlServerMessage::Error {
                request_id: Some(request_id),
                code: ControlErrorCode::PermissionDenied,
                message: "unsupported fixture request".into(),
            },
        };
        send(&mut socket, response).await?;
    }
}

fn user() -> serde_json::Value {
    json!({"id":UserId::from_u128(9100),"username":"android-acceptance","server_admin":false})
}
fn authenticated(headers: &HeaderMap, state: &State) -> bool {
    headers.get("authorization").and_then(|v| v.to_str().ok())
        == Some(format!("Bearer {}", state.token).as_str())
}
async fn login(
    AppState(state): AppState<Arc<Mutex<State>>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let password = body["password"]
        .as_str()
        .filter(|v| v.len() >= 8 && v.len() <= 1024)
        .ok_or(StatusCode::BAD_REQUEST)?;
    if body["username"] != "android-acceptance" {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let mut lock = state.lock().await;
    if lock
        .account_password
        .as_ref()
        .is_some_and(|value| value != password)
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    lock.account_password = Some(password.into());
    Ok(Json(json!({"user":user(),"access_token":lock.token})))
}
async fn current(
    AppState(state): AppState<Arc<Mutex<State>>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if !authenticated(&headers, &*state.lock().await) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(user()))
}
async fn bind_context(
    AppState(state): AppState<Arc<Mutex<State>>>,
    headers: HeaderMap,
    Json(update): Json<EndpointUserContextUpdate>,
) -> Result<Json<EndpointUserContextReceipt>, StatusCode> {
    let mut lock = state.lock().await;
    if !lock.endpoints.contains_key(&update.endpoint_key) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let logged_in = authenticated(&headers, &lock);
    let digest = if logged_in {
        blake3::hash(lock.token.as_bytes()).to_hex().to_string()
    } else {
        String::new()
    };
    PublicKey::from_bytes(update.endpoint_key.as_bytes())
        .map_err(|_| StatusCode::UNAUTHORIZED)?
        .verify(
            &update.signing_message(&lock.origin, &digest),
            &Signature::from_bytes(&update.signature.to_bytes()),
        )
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let context = EndpointUserContext {
        revision: update.revision,
        user: logged_in.then(|| UserAttribution {
            user_id: UserId::from_u128(9100),
            username: "android-acceptance".into(),
        }),
        policy_version: 1,
    };
    lock.contexts.insert(update.endpoint_key, context.clone());
    Ok(Json(EndpointUserContextReceipt {
        context,
        relay_nodes: vec![],
    }))
}
async fn websocket(
    AppState(state): AppState<Arc<Mutex<State>>>,
    upgrade: WebSocketUpgrade,
) -> axum::response::Response {
    upgrade.on_upgrade(move |socket| async move {
        let _ = handle(socket, state).await;
    })
}
#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])?;
    let tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
        )?;
    let mut relay = iroh_relay::server::RelayConfig::new(([127, 0, 0, 1], 0));
    relay.tls = Some(iroh_relay::server::TlsConfig::new(
        ([127, 0, 0, 1], 0),
        iroh_relay::server::CertConfig::Manual {
            server_config: tls.clone(),
        },
    ));
    let mut config = iroh_relay::server::ServerConfig::default();
    config.relay = Some(relay);
    let relay = iroh_relay::server::Server::spawn(config).await?;
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let state = Arc::new(Mutex::new(State {
        token: iroh_base::SecretKey::generate()
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        origin: format!("https://localhost:{port}"),
        ..Default::default()
    }));
    let app = axum::Router::new()
        .route("/control", get(websocket))
        .route("/api/account/register", post(login))
        .route("/api/account/session", post(login).get(current))
        .route("/api/account/logout", post(|| async { StatusCode::OK }))
        .route("/api/account/endpoint-context", put(bind_context))
        .with_state(state);
    let config = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(tls));
    emit(
        json!({"ready":true,"control_port":port,"relay_port":relay.https_addr().ok_or("no relay")?.port(),"ca_pem":cert.cert.pem()}),
    );
    axum_server::from_tcp_rustls(listener, config)?
        .serve(app.into_make_service())
        .await?;
    Ok(())
}
