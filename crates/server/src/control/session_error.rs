use pab_protocol::{ControlErrorCode, ControlServerMessage, RequestId};

use crate::{EndpointProofError, ServiceError, StoreError};

impl ControlSessionError {
    pub(super) fn endpoint_authentication(error: ServiceError) -> Self {
        match error {
            ServiceError::Store(StoreError::NotFound)
            | ServiceError::WrongEndpointProofPurpose
            | ServiceError::WrongEndpointProofPrincipal
            | ServiceError::EndpointIdentityChanged => Self::EndpointAuthenticationFailed,
            other => Self::Service(other),
        }
    }
}

pub(super) fn error_response(
    request_id: Option<RequestId>,
    error: ControlSessionError,
) -> ControlServerMessage {
    let (code, message) = match error {
        ControlSessionError::NotAuthenticated => (
            ControlErrorCode::InvalidState,
            "account login is required".to_owned(),
        ),
        ControlSessionError::AlreadyAuthenticated => (
            ControlErrorCode::InvalidState,
            "this connection is already authenticated".to_owned(),
        ),
        ControlSessionError::RegistrationDisabled => (
            ControlErrorCode::RegistrationDisabled,
            "account registration is disabled".to_owned(),
        ),
        ControlSessionError::NoPendingRegistration => (
            ControlErrorCode::InvalidState,
            "no endpoint registration is pending".to_owned(),
        ),
        ControlSessionError::NoPendingAuthentication => (
            ControlErrorCode::InvalidState,
            "no endpoint authentication is pending".to_owned(),
        ),
        ControlSessionError::ProofPending => (
            ControlErrorCode::InvalidState,
            "an endpoint proof challenge is already pending".to_owned(),
        ),
        ControlSessionError::RateLimited => (
            ControlErrorCode::RateLimited,
            "too many attempts on this connection".to_owned(),
        ),
        ControlSessionError::EndpointAuthenticationFailed => (
            ControlErrorCode::InvalidCredentials,
            "endpoint authentication failed".to_owned(),
        ),
        ControlSessionError::DeviceEndpointRequired => (
            ControlErrorCode::InvalidState,
            "an authenticated device endpoint is required".to_owned(),
        ),
        ControlSessionError::UserEndpointRequired => (
            ControlErrorCode::InvalidState,
            "an authenticated user endpoint is required".to_owned(),
        ),
        ControlSessionError::DeviceIdentityMismatch => (
            ControlErrorCode::PermissionDenied,
            "device identity does not match this connection".to_owned(),
        ),
        ControlSessionError::EndpointProof(_) => (
            ControlErrorCode::InvalidCredentials,
            "endpoint proof was rejected".to_owned(),
        ),
        ControlSessionError::Service(ServiceError::InvalidCredentials) => (
            ControlErrorCode::InvalidCredentials,
            "invalid username or password".to_owned(),
        ),
        ControlSessionError::Service(ServiceError::WrongEndpointProofPurpose) => (
            ControlErrorCode::InvalidMessage,
            "endpoint proof purpose does not match the operation".to_owned(),
        ),
        ControlSessionError::Service(ServiceError::WrongEndpointProofPrincipal) => (
            ControlErrorCode::InvalidMessage,
            "endpoint proof principal does not match the operation".to_owned(),
        ),
        ControlSessionError::Service(ServiceError::EndpointIdentityChanged) => (
            ControlErrorCode::InvalidCredentials,
            "endpoint authentication failed".to_owned(),
        ),
        ControlSessionError::Service(ServiceError::DeviceEndpointRequired) => (
            ControlErrorCode::InvalidState,
            "an authenticated device endpoint is required".to_owned(),
        ),
        ControlSessionError::Service(ServiceError::UserEndpointRequired) => (
            ControlErrorCode::InvalidState,
            "an authenticated user endpoint is required".to_owned(),
        ),
        ControlSessionError::Service(ServiceError::PeerEndpointOffline) => (
            ControlErrorCode::NotFound,
            "peer endpoint is unavailable".to_owned(),
        ),
        ControlSessionError::Service(ServiceError::UnsupportedDeviceSessionSchema(_))
        | ControlSessionError::Service(ServiceError::UnsupportedDeviceNetworkSchema(_))
        | ControlSessionError::Service(ServiceError::InvalidAgentVersion)
        | ControlSessionError::Service(ServiceError::InvalidObservedTime)
        | ControlSessionError::Service(ServiceError::InvalidAddressRevision)
        | ControlSessionError::Service(ServiceError::TooManyDeviceAddresses)
        | ControlSessionError::Service(ServiceError::InvalidDeviceRelayUrl)
        | ControlSessionError::Service(ServiceError::InvalidDeviceDirectAddress)
        | ControlSessionError::Service(ServiceError::TaskRuntime(_)) => (
            ControlErrorCode::InvalidMessage,
            "device hello is invalid".to_owned(),
        ),
        ControlSessionError::Service(ServiceError::Store(StoreError::InvalidInput(_))) => (
            ControlErrorCode::InvalidMessage,
            "request input is invalid".to_owned(),
        ),
        ControlSessionError::Service(ServiceError::Store(StoreError::InvalidState(_))) => (
            ControlErrorCode::InvalidState,
            "resource is not in the required state".to_owned(),
        ),
        ControlSessionError::Service(ServiceError::Store(StoreError::PermissionDenied)) => (
            ControlErrorCode::PermissionDenied,
            "operation is not permitted in this tenant".to_owned(),
        ),
        ControlSessionError::Service(ServiceError::Store(StoreError::Conflict(_))) => (
            ControlErrorCode::Conflict,
            "resource already exists".to_owned(),
        ),
        ControlSessionError::Service(ServiceError::Store(StoreError::NotFound)) => (
            ControlErrorCode::NotFound,
            "resource was not found".to_owned(),
        ),
        ControlSessionError::Service(ServiceError::Credential(error)) => {
            (ControlErrorCode::InvalidMessage, error.to_string())
        }
        ControlSessionError::Service(_) => (
            ControlErrorCode::Internal,
            "control service failed".to_owned(),
        ),
    };
    ControlServerMessage::Error {
        request_id,
        code,
        message,
    }
}

#[derive(Debug, thiserror::Error)]
pub(super) enum ControlSessionError {
    #[error("too many attempts on this connection")]
    RateLimited,
    #[error("account login is required")]
    NotAuthenticated,
    #[error("connection is already authenticated")]
    AlreadyAuthenticated,
    #[error("account registration is disabled")]
    RegistrationDisabled,
    #[error("no endpoint registration is pending")]
    NoPendingRegistration,
    #[error("no endpoint authentication is pending")]
    NoPendingAuthentication,
    #[error("an endpoint proof challenge is already pending")]
    ProofPending,
    #[error("endpoint authentication failed")]
    EndpointAuthenticationFailed,
    #[error("an authenticated device endpoint is required")]
    DeviceEndpointRequired,
    #[error("an authenticated user endpoint is required")]
    UserEndpointRequired,
    #[error("device identity does not match this connection")]
    DeviceIdentityMismatch,
    #[error(transparent)]
    EndpointProof(#[from] EndpointProofError),
    #[error(transparent)]
    Service(#[from] ServiceError),
}
