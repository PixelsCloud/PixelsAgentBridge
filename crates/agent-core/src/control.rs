use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use iroh_base::SecretKey;
use pab_protocol::{
    AuthorizedDevicePeer, ControlClientMessage, ControlErrorCode, ControlServerMessage,
    DeploymentId, DeviceHello, DeviceHelloResult, DeviceNetworkResult, DeviceNetworkSnapshot,
    DeviceNetworkUpdate, DeviceRef, ENDPOINT_PROOF_CLOCK_SKEW_MS, ENDPOINT_PROOF_SCHEMA_VERSION,
    EndpointAuthenticationResult, EndpointKey, EndpointProofChallenge, EndpointProofPrincipal,
    EndpointProofPurpose, EndpointProofResponse, EndpointSignature, RequestId, TenantId,
};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
    tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig},
};

const MAX_CONTROL_MESSAGE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct EndpointControlConfig {
    pub url: String,
    pub deployment_id: DeploymentId,
    pub tenant_id: TenantId,
    pub principal: EndpointProofPrincipal,
    pub operation_timeout: Duration,
}

pub struct AuthenticatedControlConnection {
    identity: EndpointAuthenticationResult,
    pub(crate) socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl AuthenticatedControlConnection {
    pub async fn approve_device_claim(
        &mut self,
        claim_id: pab_protocol::ClaimId,
        timeout: Duration,
    ) -> Result<(pab_protocol::DeviceId, TenantId), EndpointControlError> {
        let request_id = RequestId::new();
        send(
            &mut self.socket,
            &ControlClientMessage::ApproveDeviceClaim {
                request_id,
                claim_id,
            },
            timeout,
        )
        .await?;
        match receive(&mut self.socket, timeout).await? {
            ControlServerMessage::DeviceClaimApproved {
                request_id: result_id,
                claim_id: result_claim,
                device_id,
                owner_tenant_id,
            } if result_id == request_id && result_claim == claim_id => {
                Ok((device_id, owner_tenant_id))
            }
            ControlServerMessage::Error {
                request_id: Some(result_id),
                code,
                message,
            } if result_id == request_id => Err(EndpointControlError::Server { code, message }),
            _ => Err(EndpointControlError::MismatchedResponse),
        }
    }
    pub async fn connect(
        config: &EndpointControlConfig,
        secret: &SecretKey,
        connector: Connector,
    ) -> Result<Self, EndpointControlError> {
        if config.operation_timeout.is_zero() {
            return Err(EndpointControlError::InvalidTimeout);
        }
        let request = config.url.as_str().into_client_request()?;
        if request.uri().scheme_str() != Some("wss") {
            return Err(EndpointControlError::TlsRequired);
        }
        let endpoint_key = EndpointKey::new(*secret.public().as_bytes());
        let websocket_config = WebSocketConfig::default()
            .max_message_size(Some(MAX_CONTROL_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_CONTROL_MESSAGE_BYTES));
        let (mut socket, _) = tokio::time::timeout(
            config.operation_timeout,
            connect_async_tls_with_config(request, Some(websocket_config), false, Some(connector)),
        )
        .await
        .map_err(|_| EndpointControlError::Timeout)??;

        let begin_request_id = RequestId::new();
        send(
            &mut socket,
            &ControlClientMessage::BeginEndpointAuthentication {
                request_id: begin_request_id,
                endpoint_key,
            },
            config.operation_timeout,
        )
        .await?;
        let challenge = match receive(&mut socket, config.operation_timeout).await? {
            ControlServerMessage::EndpointChallenge {
                request_id,
                challenge,
            } if request_id == begin_request_id => challenge,
            ControlServerMessage::Error {
                request_id: Some(request_id),
                code,
                message,
            } if request_id == begin_request_id => {
                return Err(EndpointControlError::Server { code, message });
            }
            _ => return Err(EndpointControlError::MismatchedResponse),
        };
        validate_challenge(config, endpoint_key, &challenge)?;

        let complete_request_id = RequestId::new();
        send(
            &mut socket,
            &ControlClientMessage::CompleteEndpointAuthentication {
                request_id: complete_request_id,
                proof: EndpointProofResponse {
                    challenge_id: challenge.challenge_id,
                    signature: EndpointSignature::from_bytes(
                        secret.sign(&challenge.signing_message()).to_bytes(),
                    ),
                },
            },
            config.operation_timeout,
        )
        .await?;
        let identity = match receive(&mut socket, config.operation_timeout).await? {
            ControlServerMessage::EndpointAuthenticated { request_id, result }
                if request_id == complete_request_id =>
            {
                result
            }
            ControlServerMessage::Error {
                request_id: Some(request_id),
                code,
                message,
            } if request_id == complete_request_id => {
                return Err(EndpointControlError::Server { code, message });
            }
            _ => return Err(EndpointControlError::MismatchedResponse),
        };
        let expected = EndpointAuthenticationResult {
            tenant_id: config.tenant_id,
            endpoint_key,
            principal: config.principal,
        };
        if identity != expected {
            return Err(EndpointControlError::IdentityMismatch);
        }
        Ok(Self { identity, socket })
    }

    pub const fn identity(&self) -> EndpointAuthenticationResult {
        self.identity
    }

    pub async fn publish_device_hello(
        &mut self,
        hello: &DeviceHello,
        timeout: Duration,
    ) -> Result<DeviceHelloResult, EndpointControlError> {
        if timeout.is_zero() {
            return Err(EndpointControlError::InvalidTimeout);
        }
        let request_id = RequestId::new();
        send(
            &mut self.socket,
            &ControlClientMessage::PublishDeviceHello {
                request_id,
                hello: Box::new(hello.clone()),
            },
            timeout,
        )
        .await?;
        match receive(&mut self.socket, timeout).await? {
            ControlServerMessage::DeviceHelloAccepted {
                request_id: response_id,
                result,
            } if response_id == request_id
                && result.device_ref == hello.device_ref
                && result.environment_revision == hello.execution_context.environment_revision =>
            {
                Ok(result)
            }
            ControlServerMessage::Error {
                request_id: Some(response_id),
                code,
                message,
            } if response_id == request_id => Err(EndpointControlError::Server { code, message }),
            _ => Err(EndpointControlError::MismatchedResponse),
        }
    }

    pub async fn publish_device_network(
        &mut self,
        update: &DeviceNetworkUpdate,
        timeout: Duration,
    ) -> Result<DeviceNetworkResult, EndpointControlError> {
        if timeout.is_zero() {
            return Err(EndpointControlError::InvalidTimeout);
        }
        let request_id = RequestId::new();
        send(
            &mut self.socket,
            &ControlClientMessage::PublishDeviceNetwork {
                request_id,
                update: Box::new(update.clone()),
            },
            timeout,
        )
        .await?;
        match receive(&mut self.socket, timeout).await? {
            ControlServerMessage::DeviceNetworkAccepted {
                request_id: response_id,
                result,
            } if response_id == request_id
                && result.device_ref == update.device_ref
                && result.endpoint_instance_id == update.endpoint_instance_id
                && result.address_revision == update.address_revision =>
            {
                Ok(result)
            }
            ControlServerMessage::Error {
                request_id: Some(response_id),
                code,
                message,
            } if response_id == request_id => Err(EndpointControlError::Server { code, message }),
            _ => Err(EndpointControlError::MismatchedResponse),
        }
    }

    pub async fn get_device_network(
        &mut self,
        device_ref: DeviceRef,
        timeout: Duration,
    ) -> Result<DeviceNetworkSnapshot, EndpointControlError> {
        if timeout.is_zero() {
            return Err(EndpointControlError::InvalidTimeout);
        }
        let request_id = RequestId::new();
        send(
            &mut self.socket,
            &ControlClientMessage::GetDeviceNetwork {
                request_id,
                device_ref,
            },
            timeout,
        )
        .await?;
        match receive(&mut self.socket, timeout).await? {
            ControlServerMessage::DeviceNetworkFound {
                request_id: response_id,
                snapshot,
            } if response_id == request_id && snapshot.device_ref == device_ref => Ok(*snapshot),
            ControlServerMessage::Error {
                request_id: Some(response_id),
                code,
                message,
            } if response_id == request_id => Err(EndpointControlError::Server { code, message }),
            _ => Err(EndpointControlError::MismatchedResponse),
        }
    }

    pub async fn resolve_device_code(
        &mut self,
        device_code: pab_protocol::DeviceCode,
        timeout: Duration,
    ) -> Result<DeviceRef, EndpointControlError> {
        if timeout.is_zero() {
            return Err(EndpointControlError::InvalidTimeout);
        }
        let request_id = RequestId::new();
        send(
            &mut self.socket,
            &ControlClientMessage::ResolveDeviceCode {
                request_id,
                device_code,
            },
            timeout,
        )
        .await?;
        match receive(&mut self.socket, timeout).await? {
            ControlServerMessage::DeviceCodeResolved {
                request_id: response_id,
                device_ref,
            } if response_id == request_id => Ok(device_ref),
            ControlServerMessage::Error {
                request_id: Some(response_id),
                code,
                message,
            } if response_id == request_id => Err(EndpointControlError::Server { code, message }),
            _ => Err(EndpointControlError::MismatchedResponse),
        }
    }

    pub async fn authorize_device_peer(
        &mut self,
        peer_endpoint_key: EndpointKey,
        timeout: Duration,
    ) -> Result<AuthorizedDevicePeer, EndpointControlError> {
        if timeout.is_zero() {
            return Err(EndpointControlError::InvalidTimeout);
        }
        let request_id = RequestId::new();
        send(
            &mut self.socket,
            &ControlClientMessage::AuthorizeDevicePeer {
                request_id,
                peer_endpoint_key,
            },
            timeout,
        )
        .await?;
        match receive(&mut self.socket, timeout).await? {
            ControlServerMessage::DevicePeerAuthorized {
                request_id: response_id,
                result,
            } if response_id == request_id && result.peer_endpoint_key == peer_endpoint_key => {
                Ok(result)
            }
            ControlServerMessage::Error {
                request_id: Some(response_id),
                code,
                message,
            } if response_id == request_id => Err(EndpointControlError::Server { code, message }),
            _ => Err(EndpointControlError::MismatchedResponse),
        }
    }
}

fn validate_challenge(
    config: &EndpointControlConfig,
    endpoint_key: EndpointKey,
    challenge: &EndpointProofChallenge,
) -> Result<(), EndpointControlError> {
    if challenge.schema_version != ENDPOINT_PROOF_SCHEMA_VERSION
        || challenge.deployment_id != config.deployment_id
        || challenge.tenant_id != config.tenant_id
        || challenge.principal != config.principal
        || challenge.endpoint_key != endpoint_key
        || challenge.purpose != EndpointProofPurpose::AuthenticateRegisteredEndpoint
    {
        return Err(EndpointControlError::ChallengeMismatch);
    }
    challenge
        .validate_at_with_skew(
            unix_millis(SystemTime::now())?,
            ENDPOINT_PROOF_CLOCK_SKEW_MS,
        )
        .map_err(|_| EndpointControlError::InvalidChallenge)?;
    Ok(())
}

pub(crate) async fn send(
    socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    message: &ControlClientMessage,
    timeout: Duration,
) -> Result<(), EndpointControlError> {
    let encoded = serde_json::to_string(message)?;
    tokio::time::timeout(timeout, socket.send(Message::Text(encoded.into())))
        .await
        .map_err(|_| EndpointControlError::Timeout)??;
    Ok(())
}

pub(crate) async fn receive(
    socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    timeout: Duration,
) -> Result<ControlServerMessage, EndpointControlError> {
    loop {
        let message = tokio::time::timeout(timeout, socket.next())
            .await
            .map_err(|_| EndpointControlError::Timeout)?
            .ok_or(EndpointControlError::Closed)??;
        match message {
            Message::Text(text) => return Ok(serde_json::from_str(text.as_str())?),
            Message::Ping(payload) => socket.send(Message::Pong(payload)).await?,
            Message::Pong(_) => {}
            Message::Close(_) => return Err(EndpointControlError::Closed),
            Message::Binary(_) | Message::Frame(_) => {
                return Err(EndpointControlError::UnexpectedMessage);
            }
        }
    }
}

fn unix_millis(value: SystemTime) -> Result<i64, EndpointControlError> {
    let duration = value
        .duration_since(UNIX_EPOCH)
        .map_err(|_| EndpointControlError::InvalidSystemTime)?;
    i64::try_from(duration.as_millis()).map_err(|_| EndpointControlError::InvalidSystemTime)
}

#[derive(Debug, Error)]
pub enum EndpointControlError {
    #[error("endpoint control requires a wss:// URL")]
    TlsRequired,
    #[error("endpoint control timeout must be greater than zero")]
    InvalidTimeout,
    #[error("endpoint control operation timed out")]
    Timeout,
    #[error("endpoint control connection closed")]
    Closed,
    #[error("endpoint control returned an unexpected WebSocket message")]
    UnexpectedMessage,
    #[error("endpoint control response does not match its request")]
    MismatchedResponse,
    #[error("endpoint proof challenge does not match the configured identity")]
    ChallengeMismatch,
    #[error("endpoint proof challenge is not currently valid")]
    InvalidChallenge,
    #[error("authenticated endpoint identity does not match the configured identity")]
    IdentityMismatch,
    #[error("system time is outside the supported Unix timestamp range")]
    InvalidSystemTime,
    #[error("endpoint control was rejected with {code:?}: {message}")]
    Server {
        code: ControlErrorCode,
        message: String,
    },
    #[error(transparent)]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use pab_protocol::{ChallengeId, ConnectionId, DeviceId};

    fn config(principal: EndpointProofPrincipal) -> EndpointControlConfig {
        EndpointControlConfig {
            url: "wss://localhost/control".to_owned(),
            deployment_id: DeploymentId::from_u128(1),
            tenant_id: TenantId::from_u128(2),
            principal,
            operation_timeout: Duration::from_secs(1),
        }
    }

    #[test]
    fn rejects_a_challenge_for_another_tenant() {
        let secret = SecretKey::generate();
        let principal = EndpointProofPrincipal::Device {
            device_id: DeviceId::from_u128(3),
        };
        let config = config(principal);
        let now = unix_millis(SystemTime::now()).unwrap();
        let challenge = EndpointProofChallenge {
            schema_version: ENDPOINT_PROOF_SCHEMA_VERSION,
            challenge_id: ChallengeId::from_u128(4),
            deployment_id: config.deployment_id,
            connection_id: ConnectionId::from_u128(5),
            principal,
            tenant_id: TenantId::from_u128(99),
            endpoint_key: EndpointKey::new(*secret.public().as_bytes()),
            purpose: EndpointProofPurpose::AuthenticateRegisteredEndpoint,
            issued_at_unix_ms: now,
            expires_at_unix_ms: now + 30_000,
            nonce: [6; 32],
        };

        assert!(matches!(
            validate_challenge(
                &config,
                EndpointKey::new(*secret.public().as_bytes()),
                &challenge
            ),
            Err(EndpointControlError::ChallengeMismatch)
        ));
    }
}
