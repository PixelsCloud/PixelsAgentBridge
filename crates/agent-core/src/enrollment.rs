use std::time::Duration;

use iroh_base::SecretKey;
use pab_protocol::{
    ControlClientMessage, ControlServerMessage, DeploymentId, ENDPOINT_PROOF_CLOCK_SKEW_MS,
    EndpointKey, EndpointProofChallenge, EndpointProofPrincipal, EndpointProofPurpose,
    EndpointProofResponse, EndpointRegistration, EndpointRegistrationResult, EndpointSignature,
    RequestId, TenantId, UserId,
};
use thiserror::Error;
use tokio_tungstenite::{
    Connector, connect_async_tls_with_config,
    tungstenite::{client::IntoClientRequest, protocol::WebSocketConfig},
};
use zeroize::{Zeroize, Zeroizing};

use crate::control::{receive, send};

const MAX_CONTROL_MESSAGE_BYTES: usize = 64 * 1024;

pub struct EndpointEnrollment {
    pub user_id: UserId,
    pub tenant_id: TenantId,
    pub device_id: pab_protocol::DeviceId,
    pub user_endpoint_secret: SecretKey,
    pub device_endpoint_secret: SecretKey,
}

pub async fn enroll_account_with_device(
    control_url: &str,
    deployment_id: DeploymentId,
    username: String,
    password: Zeroizing<String>,
    device_name: String,
    connector: Connector,
    timeout: Duration,
) -> Result<EndpointEnrollment, EnrollmentError> {
    if !control_url.starts_with("wss://") || timeout.is_zero() {
        return Err(EnrollmentError::InvalidConfig);
    }
    let request = control_url.into_client_request()?;
    let websocket_config = WebSocketConfig::default()
        .max_message_size(Some(MAX_CONTROL_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_CONTROL_MESSAGE_BYTES));
    let (mut socket, _) = tokio::time::timeout(
        timeout,
        connect_async_tls_with_config(request, Some(websocket_config), false, Some(connector)),
    )
    .await
    .map_err(|_| EnrollmentError::Timeout)??;

    let account_request = RequestId::new();
    let mut account_message = ControlClientMessage::RegisterAccount {
        request_id: account_request,
        username,
        password: password.to_string(),
    };
    send(&mut socket, &account_message, timeout).await?;
    if let ControlClientMessage::RegisterAccount { password, .. } = &mut account_message {
        password.zeroize();
    }
    let (user_id, tenant_id) = match receive(&mut socket, timeout).await? {
        ControlServerMessage::AccountAuthenticated {
            request_id,
            user_id,
            personal_tenant_id,
            ..
        } if request_id == account_request => (user_id, personal_tenant_id),
        response => return Err(response_error(response, account_request)),
    };

    let user_endpoint_secret = SecretKey::generate();
    let user_result = register_endpoint(
        &mut socket,
        deployment_id,
        tenant_id,
        user_id,
        &user_endpoint_secret,
        EndpointRegistration::User,
        timeout,
    )
    .await?;
    match user_result {
        EndpointRegistrationResult::User {
            tenant_id: result_tenant,
            endpoint_key,
        } if result_tenant == tenant_id
            && endpoint_key == EndpointKey::new(*user_endpoint_secret.public().as_bytes()) => {}
        _ => return Err(EnrollmentError::MismatchedResponse),
    }

    let device_endpoint_secret = SecretKey::generate();
    let device_result = register_endpoint(
        &mut socket,
        deployment_id,
        tenant_id,
        user_id,
        &device_endpoint_secret,
        EndpointRegistration::Device { name: device_name },
        timeout,
    )
    .await?;
    let device_id = match device_result {
        EndpointRegistrationResult::Device {
            tenant_id: result_tenant,
            device_id,
            endpoint_key,
        } if result_tenant == tenant_id
            && endpoint_key == EndpointKey::new(*device_endpoint_secret.public().as_bytes()) =>
        {
            device_id
        }
        _ => return Err(EnrollmentError::MismatchedResponse),
    };
    Ok(EndpointEnrollment {
        user_id,
        tenant_id,
        device_id,
        user_endpoint_secret,
        device_endpoint_secret,
    })
}

async fn register_endpoint(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    deployment_id: DeploymentId,
    tenant_id: TenantId,
    user_id: UserId,
    secret: &SecretKey,
    registration: EndpointRegistration,
    timeout: Duration,
) -> Result<EndpointRegistrationResult, EnrollmentError> {
    let endpoint_key = EndpointKey::new(*secret.public().as_bytes());
    let purpose = match registration {
        EndpointRegistration::User => EndpointProofPurpose::RegisterUserEndpoint,
        EndpointRegistration::Device { .. } => EndpointProofPurpose::RegisterDevice,
    };
    let begin_id = RequestId::new();
    send(
        socket,
        &ControlClientMessage::BeginEndpointRegistration {
            request_id: begin_id,
            tenant_id,
            endpoint_key,
            registration,
        },
        timeout,
    )
    .await?;
    let challenge = match receive(socket, timeout).await? {
        ControlServerMessage::EndpointChallenge {
            request_id,
            challenge,
        } if request_id == begin_id => challenge,
        response => return Err(response_error(response, begin_id)),
    };
    validate_challenge(
        &challenge,
        deployment_id,
        tenant_id,
        user_id,
        endpoint_key,
        purpose,
    )?;
    let complete_id = RequestId::new();
    send(
        socket,
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
    match receive(socket, timeout).await? {
        ControlServerMessage::EndpointRegistered { request_id, result }
            if request_id == complete_id =>
        {
            Ok(result)
        }
        response => Err(response_error(response, complete_id)),
    }
}

fn validate_challenge(
    challenge: &EndpointProofChallenge,
    deployment_id: DeploymentId,
    tenant_id: TenantId,
    user_id: UserId,
    endpoint_key: EndpointKey,
    purpose: EndpointProofPurpose,
) -> Result<(), EnrollmentError> {
    if challenge.deployment_id != deployment_id {
        return Err(EnrollmentError::ChallengeMismatch("deployment"));
    }
    if challenge.tenant_id != tenant_id {
        return Err(EnrollmentError::ChallengeMismatch("tenant"));
    }
    if challenge.principal != (EndpointProofPrincipal::User { user_id }) {
        return Err(EnrollmentError::ChallengeMismatch("principal"));
    }
    if challenge.endpoint_key != endpoint_key {
        return Err(EnrollmentError::ChallengeMismatch("endpoint key"));
    }
    if challenge.purpose != purpose {
        return Err(EnrollmentError::ChallengeMismatch("purpose"));
    }
    let now = time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
    let now = i64::try_from(now).map_err(|_| EnrollmentError::ChallengeMismatch("clock"))?;
    challenge
        .validate_at_with_skew(now, ENDPOINT_PROOF_CLOCK_SKEW_MS)
        .map_err(|_| EnrollmentError::ChallengeMismatch("validity"))
}

fn response_error(response: ControlServerMessage, request_id: RequestId) -> EnrollmentError {
    match response {
        ControlServerMessage::Error {
            request_id: Some(response_id),
            code,
            message,
        } if response_id == request_id => EnrollmentError::Server { code, message },
        _ => EnrollmentError::MismatchedResponse,
    }
}

#[derive(Debug, Error)]
pub enum EnrollmentError {
    #[error("enrollment requires a WSS URL and a nonzero timeout")]
    InvalidConfig,
    #[error("enrollment connection timed out")]
    Timeout,
    #[error("server returned a response for another enrollment request")]
    MismatchedResponse,
    #[error("server returned an invalid endpoint proof challenge ({0})")]
    ChallengeMismatch(&'static str),
    #[error("server rejected enrollment with {code:?}: {message}")]
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
