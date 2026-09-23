use std::time::Duration;

use iroh_base::SecretKey;
use pab_protocol::{
    ControlClientMessage, ControlServerMessage, DeploymentId, EndpointKey, EndpointProofPrincipal,
    EndpointProofPurpose, EndpointProofResponse, EndpointRegistrationResult, EndpointSignature,
    RequestId,
};
use thiserror::Error;
use tokio_tungstenite::{
    Connector, connect_async_tls_with_config, tungstenite::client::IntoClientRequest,
};

use crate::control::{receive, send};

pub enum OpenRegistrationKind {
    Device { name: String },
    Guest,
}

pub async fn register_open_endpoint(
    url: &str,
    deployment_id: DeploymentId,
    secret: &SecretKey,
    kind: OpenRegistrationKind,
    connector: Connector,
    timeout: Duration,
) -> Result<EndpointRegistrationResult, OpenRegistrationError> {
    if !url.starts_with("wss://") || timeout.is_zero() {
        return Err(OpenRegistrationError::InvalidConfig);
    }
    let request = url.into_client_request()?;
    let (mut socket, _) = tokio::time::timeout(
        timeout,
        connect_async_tls_with_config(request, None, false, Some(connector)),
    )
    .await
    .map_err(|_| OpenRegistrationError::Timeout)??;
    let endpoint_key = EndpointKey::new(*secret.public().as_bytes());
    let (begin, purpose) = match kind {
        OpenRegistrationKind::Device { name } => (
            ControlClientMessage::BeginUnclaimedDeviceRegistration {
                request_id: RequestId::new(),
                endpoint_key,
                name,
            },
            EndpointProofPurpose::RegisterUnclaimedDevice,
        ),
        OpenRegistrationKind::Guest => (
            ControlClientMessage::BeginGuestEndpointRegistration {
                request_id: RequestId::new(),
                endpoint_key,
            },
            EndpointProofPurpose::RegisterGuestEndpoint,
        ),
    };
    let begin_id = begin.request_id();
    send(&mut socket, &begin, timeout).await?;
    let challenge = match receive(&mut socket, timeout).await? {
        ControlServerMessage::EndpointChallenge {
            request_id,
            challenge,
        } if request_id == begin_id => challenge,
        ControlServerMessage::Error {
            request_id: Some(request_id),
            code,
            message,
        } if request_id == begin_id => return Err(OpenRegistrationError::Server { code, message }),
        _ => return Err(OpenRegistrationError::MismatchedResponse),
    };
    let principal_ok = matches!(
        (purpose, challenge.principal),
        (
            EndpointProofPurpose::RegisterUnclaimedDevice,
            EndpointProofPrincipal::Device { .. }
        ) | (
            EndpointProofPurpose::RegisterGuestEndpoint,
            EndpointProofPrincipal::Guest
        )
    );
    let now = time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
    if challenge.deployment_id != deployment_id
        || challenge.endpoint_key != endpoint_key
        || challenge.purpose != purpose
        || !principal_ok
        || challenge
            .validate_at_with_skew(
                i64::try_from(now).map_err(|_| OpenRegistrationError::InvalidChallenge)?,
                pab_protocol::ENDPOINT_PROOF_CLOCK_SKEW_MS,
            )
            .is_err()
    {
        return Err(OpenRegistrationError::InvalidChallenge);
    }
    let complete_id = RequestId::new();
    send(
        &mut socket,
        &ControlClientMessage::CompleteEndpointRegistration {
            request_id: complete_id,
            proof: EndpointProofResponse {
                challenge_id: challenge.challenge_id,
                signature: EndpointSignature::from_bytes(
                    secret.sign(&challenge.signing_message()).to_bytes(),
                ),
            },
        },
        timeout,
    )
    .await?;
    match receive(&mut socket, timeout).await? {
        ControlServerMessage::EndpointRegistered { request_id, result }
            if request_id == complete_id =>
        {
            let valid = match (&result, purpose) {
                (
                    EndpointRegistrationResult::Device {
                        endpoint_key: key, ..
                    },
                    EndpointProofPurpose::RegisterUnclaimedDevice,
                ) => *key == endpoint_key,
                (
                    EndpointRegistrationResult::Guest {
                        endpoint_key: key, ..
                    },
                    EndpointProofPurpose::RegisterGuestEndpoint,
                ) => *key == endpoint_key,
                _ => false,
            };
            if valid {
                Ok(result)
            } else {
                Err(OpenRegistrationError::MismatchedResponse)
            }
        }
        ControlServerMessage::Error {
            request_id: Some(request_id),
            code,
            message,
        } if request_id == complete_id => Err(OpenRegistrationError::Server { code, message }),
        _ => Err(OpenRegistrationError::MismatchedResponse),
    }
}

#[derive(Debug, Error)]
pub enum OpenRegistrationError {
    #[error("open endpoint registration requires WSS and a nonzero timeout")]
    InvalidConfig,
    #[error("open endpoint registration timed out")]
    Timeout,
    #[error("open endpoint registration received an invalid challenge")]
    InvalidChallenge,
    #[error("open endpoint registration response did not match the request")]
    MismatchedResponse,
    #[error("server rejected registration with {code:?}: {message}")]
    Server {
        code: pab_protocol::ControlErrorCode,
        message: String,
    },
    #[error(transparent)]
    Control(#[from] crate::EndpointControlError),
    #[error(transparent)]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error(transparent)]
    Http(#[from] tokio_tungstenite::tungstenite::http::Error),
}
