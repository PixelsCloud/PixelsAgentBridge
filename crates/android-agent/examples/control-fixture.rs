//! Local TLS/proof fixture for Android acceptance. It is not a production server.
//! Deliberately closes the first authenticated session to exercise reconnection.

use futures_util::{SinkExt, StreamExt};
use iroh_base::{PublicKey, Signature};
use pab_protocol::*;
use serde_json::json;
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::net::TcpStream;
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<tokio_rustls::server::TlsStream<TcpStream>>;
type Error = Box<dyn std::error::Error + Send + Sync>;

fn emit(value: serde_json::Value) {
    println!("{value}");
    let _ = std::io::stdout().flush();
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

async fn receive(socket: &mut Socket) -> Result<ControlClientMessage, Error> {
    let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await?
        .ok_or("connection closed")??;
    let Message::Text(text) = message else {
        return Err("expected JSON".into());
    };
    Ok(serde_json::from_str(text.as_str())?)
}

async fn send(socket: &mut Socket, message: ControlServerMessage) -> Result<(), Error> {
    socket
        .send(Message::Text(serde_json::to_string(&message)?.into()))
        .await?;
    Ok(())
}

async fn handle(
    mut socket: Socket,
    auth_count: Arc<AtomicUsize>,
    key_slot: Arc<std::sync::Mutex<Option<EndpointKey>>>,
    reject_authentication: bool,
) -> Result<(), Error> {
    let first = receive(&mut socket).await?;
    let (id, endpoint_key, purpose, registering) = match first {
        ControlClientMessage::BeginUnclaimedDeviceRegistration {
            request_id,
            endpoint_key,
            ..
        } => (
            request_id,
            endpoint_key,
            EndpointProofPurpose::RegisterUnclaimedDevice,
            true,
        ),
        ControlClientMessage::BeginEndpointAuthentication {
            request_id,
            endpoint_key,
        } => (
            request_id,
            endpoint_key,
            EndpointProofPurpose::AuthenticateRegisteredEndpoint,
            false,
        ),
        _ => return Err("unexpected first request".into()),
    };
    if !registering && *key_slot.lock().unwrap() != Some(endpoint_key) {
        return Err("unregistered key".into());
    }
    let principal = EndpointProofPrincipal::Device {
        device_id: DeviceId::from_u128(9001),
    };
    let tenant_id = TenantId::from_u128(9002);
    let challenge = EndpointProofChallenge {
        schema_version: ENDPOINT_PROOF_SCHEMA_VERSION,
        challenge_id: ChallengeId::new(),
        connection_id: ConnectionId::new(),
        principal,
        tenant_id,
        endpoint_key,
        purpose,
        issued_at_unix_ms: now_ms() - 100,
        expires_at_unix_ms: now_ms() + 30_000,
        nonce: iroh_base::SecretKey::generate().to_bytes(),
    };
    send(
        &mut socket,
        ControlServerMessage::EndpointChallenge {
            request_id: id,
            challenge: challenge.clone(),
        },
    )
    .await?;
    let completed = receive(&mut socket).await?;
    let (request_id, proof) = match completed {
        ControlClientMessage::CompleteEndpointRegistration { request_id, proof } if registering => {
            (request_id, proof)
        }
        ControlClientMessage::CompleteEndpointAuthentication { request_id, proof }
            if !registering =>
        {
            (request_id, proof)
        }
        _ => return Err("unexpected proof".into()),
    };
    if proof.challenge_id != challenge.challenge_id {
        return Err("wrong challenge".into());
    }
    PublicKey::from_bytes(endpoint_key.as_bytes())?.verify(
        &challenge.signing_message(),
        &Signature::from_bytes(&proof.signature.to_bytes()),
    )?;
    if registering {
        *key_slot.lock().unwrap() = Some(endpoint_key);
        send(
            &mut socket,
            ControlServerMessage::EndpointRegistered {
                request_id,
                result: EndpointRegistrationResult::Device {
                    tenant_id,
                    device_id: DeviceId::from_u128(9001),
                    device_code: DeviceCode::new(987654321)?,
                    endpoint_key,
                },
            },
        )
        .await?;
        emit(json!({"event":"registered"}));
        return Ok(());
    }
    if reject_authentication {
        send(
            &mut socket,
            ControlServerMessage::Error {
                request_id: Some(request_id),
                code: ControlErrorCode::PermissionDenied,
                message: "isolated authentication rejection".into(),
            },
        )
        .await?;
        emit(json!({"event":"authentication_rejected"}));
        return Ok(());
    }
    send(
        &mut socket,
        ControlServerMessage::EndpointAuthenticated {
            request_id,
            result: EndpointAuthenticationResult {
                tenant_id,
                endpoint_key,
                principal,
            },
        },
    )
    .await?;
    let number = auth_count.fetch_add(1, Ordering::SeqCst) + 1;
    emit(json!({"event":"authenticated","session":number}));
    let disconnect = tokio::time::sleep(if number == 1 {
        Duration::from_secs(2)
    } else {
        Duration::from_secs(3600)
    });
    tokio::pin!(disconnect);
    loop {
        tokio::select! {
            _ = &mut disconnect => {
                socket.close(None).await?;
                emit(json!({"event":"forced_disconnect"}));
                return Ok(());
            }
            message = socket.next() => match message {
                Some(Ok(Message::Ping(payload))) => {
                    socket.send(Message::Pong(payload)).await?;
                    emit(json!({"event":"heartbeat"}));
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                    emit(json!({"event":"disconnected","session":number}));
                    return Ok(());
                }
                _ => return Err("unexpected authenticated request".into()),
            }
        }
    }
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Error> {
    let reject_authentication = std::env::args().any(|arg| arg == "--reject-authentication");
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])?;
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
    let tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.cert.der().clone()], key.into())?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    emit(json!({"ready":true,"port":port,"ca_pem":cert.cert.pem()}));
    let count = Arc::new(AtomicUsize::new(0));
    let key_slot = Arc::new(std::sync::Mutex::new(None));
    loop {
        let (stream, _) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let count = count.clone();
        let key_slot = key_slot.clone();
        tokio::spawn(async move {
            let result = async {
                let tls =
                    tokio::time::timeout(Duration::from_secs(5), acceptor.accept(stream)).await??;
                let socket = tokio_tungstenite::accept_async(tls).await?;
                handle(socket, count, key_slot, reject_authentication).await
            }
            .await;
            if result.is_err() {
                emit(json!({"event":"rejected"}));
            }
        });
    }
}
