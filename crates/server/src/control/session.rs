use std::time::Duration;

use pab_protocol::{
    ControlClientMessage, ControlErrorCode, ControlServerMessage, DeploymentId,
    EndpointAuthenticationResult, EndpointKey, EndpointProofPrincipal, EndpointProofPurpose,
    EndpointRegistration, EndpointRegistrationResult, RequestId,
};
use time::OffsetDateTime;

use super::ControlApiConfig;
use crate::{
    ControlPlane, EndpointProofError, EndpointProofSession, ServiceError, StoreError,
    domain::{Account, RegisteredEndpoint},
};

const ENDPOINT_CHALLENGE_VALIDITY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
enum PendingRegistration {
    User,
    Device { name: String },
}

#[derive(Debug, Clone)]
enum PendingProof {
    Registration(PendingRegistration),
    Authentication,
}

pub struct ControlSession {
    control: ControlPlane,
    config: ControlApiConfig,
    proof_session: EndpointProofSession,
    account: Option<Account>,
    endpoint: Option<RegisteredEndpoint>,
    pending_proof: Option<PendingProof>,
}

impl ControlSession {
    pub fn new(
        control: ControlPlane,
        deployment_id: DeploymentId,
        config: ControlApiConfig,
    ) -> Self {
        Self {
            control,
            config,
            proof_session: EndpointProofSession::new(deployment_id),
            account: None,
            endpoint: None,
            pending_proof: None,
        }
    }

    pub const fn is_authenticated(&self) -> bool {
        self.account.is_some() || self.endpoint.is_some()
    }

    pub async fn handle(&mut self, message: ControlClientMessage) -> ControlServerMessage {
        let request_id = message.request_id();
        let result = match message {
            ControlClientMessage::RegisterAccount {
                username, password, ..
            } => {
                self.register_account(request_id, &username, &password)
                    .await
            }
            ControlClientMessage::Login {
                username, password, ..
            } => self.login(request_id, &username, &password).await,
            ControlClientMessage::BeginEndpointRegistration {
                tenant_id,
                endpoint_key,
                registration,
                ..
            } => self
                .begin_endpoint_registration(tenant_id, endpoint_key, registration)
                .map(|challenge| ControlServerMessage::EndpointChallenge {
                    request_id,
                    challenge,
                }),
            ControlClientMessage::CompleteEndpointRegistration { proof, .. } => self
                .complete_endpoint_registration(proof)
                .await
                .map(|result| ControlServerMessage::EndpointRegistered { request_id, result }),
            ControlClientMessage::BeginEndpointAuthentication { endpoint_key, .. } => self
                .begin_endpoint_authentication(endpoint_key)
                .await
                .map(|challenge| ControlServerMessage::EndpointChallenge {
                    request_id,
                    challenge,
                }),
            ControlClientMessage::CompleteEndpointAuthentication { proof, .. } => self
                .complete_endpoint_authentication(proof)
                .await
                .map(|result| ControlServerMessage::EndpointAuthenticated { request_id, result }),
        };
        match result {
            Ok(response) => response,
            Err(error) => error_response(Some(request_id), error),
        }
    }

    async fn register_account(
        &mut self,
        request_id: RequestId,
        username: &str,
        password: &str,
    ) -> Result<ControlServerMessage, ControlSessionError> {
        if self.is_authenticated() {
            return Err(ControlSessionError::AlreadyAuthenticated);
        }
        if self.pending_proof.is_some() {
            return Err(ControlSessionError::ProofPending);
        }
        if !self.config.registration_enabled {
            return Err(ControlSessionError::RegistrationDisabled);
        }
        let account = self.control.register_account(username, password).await?;
        self.account = Some(account.clone());
        Ok(authenticated_response(request_id, account))
    }

    async fn login(
        &mut self,
        request_id: RequestId,
        username: &str,
        password: &str,
    ) -> Result<ControlServerMessage, ControlSessionError> {
        if self.is_authenticated() {
            return Err(ControlSessionError::AlreadyAuthenticated);
        }
        if self.pending_proof.is_some() {
            return Err(ControlSessionError::ProofPending);
        }
        let account = self.control.authenticate(username, password).await?;
        self.account = Some(account.clone());
        Ok(authenticated_response(request_id, account))
    }

    fn begin_endpoint_registration(
        &mut self,
        tenant_id: pab_protocol::TenantId,
        endpoint_key: pab_protocol::EndpointKey,
        registration: EndpointRegistration,
    ) -> Result<pab_protocol::EndpointProofChallenge, ControlSessionError> {
        let account = self
            .account
            .as_ref()
            .ok_or(ControlSessionError::NotAuthenticated)?;
        if self.pending_proof.is_some() {
            return Err(ControlSessionError::ProofPending);
        }
        let (purpose, pending) = match registration {
            EndpointRegistration::User => (
                EndpointProofPurpose::RegisterUserEndpoint,
                PendingRegistration::User,
            ),
            EndpointRegistration::Device { name } => (
                EndpointProofPurpose::RegisterDevice,
                PendingRegistration::Device { name },
            ),
        };
        let challenge = self.proof_session.issue(
            EndpointProofPrincipal::User {
                user_id: account.id,
            },
            tenant_id,
            endpoint_key,
            purpose,
            OffsetDateTime::now_utc(),
            ENDPOINT_CHALLENGE_VALIDITY,
        )?;
        self.pending_proof = Some(PendingProof::Registration(pending));
        Ok(challenge)
    }

    async fn complete_endpoint_registration(
        &mut self,
        response: pab_protocol::EndpointProofResponse,
    ) -> Result<EndpointRegistrationResult, ControlSessionError> {
        let pending = self
            .pending_proof
            .take()
            .ok_or(ControlSessionError::NoPendingRegistration)?;
        let PendingProof::Registration(pending) = pending else {
            return Err(ControlSessionError::NoPendingRegistration);
        };
        let proof = self
            .proof_session
            .verify(response, OffsetDateTime::now_utc())?;
        let tenant_id = proof.tenant_id();
        let endpoint_key = proof.endpoint_key();
        match pending {
            PendingRegistration::User => {
                self.control.register_user_endpoint(proof).await?;
                Ok(EndpointRegistrationResult::User {
                    tenant_id,
                    endpoint_key,
                })
            }
            PendingRegistration::Device { name } => {
                let device = self.control.register_device(proof, &name).await?;
                Ok(EndpointRegistrationResult::Device {
                    tenant_id,
                    device_id: device.id,
                    endpoint_key,
                })
            }
        }
    }

    async fn begin_endpoint_authentication(
        &mut self,
        endpoint_key: EndpointKey,
    ) -> Result<pab_protocol::EndpointProofChallenge, ControlSessionError> {
        if self.is_authenticated() {
            return Err(ControlSessionError::AlreadyAuthenticated);
        }
        if self.pending_proof.is_some() {
            return Err(ControlSessionError::ProofPending);
        }
        let endpoint = self
            .control
            .registered_endpoint(endpoint_key)
            .await
            .map_err(ControlSessionError::endpoint_authentication)?;
        let challenge = self.proof_session.issue(
            endpoint.principal,
            endpoint.tenant_id,
            endpoint.endpoint_key,
            EndpointProofPurpose::AuthenticateRegisteredEndpoint,
            OffsetDateTime::now_utc(),
            ENDPOINT_CHALLENGE_VALIDITY,
        )?;
        self.pending_proof = Some(PendingProof::Authentication);
        Ok(challenge)
    }

    async fn complete_endpoint_authentication(
        &mut self,
        response: pab_protocol::EndpointProofResponse,
    ) -> Result<EndpointAuthenticationResult, ControlSessionError> {
        if self.is_authenticated() {
            return Err(ControlSessionError::AlreadyAuthenticated);
        }
        let pending = self
            .pending_proof
            .take()
            .ok_or(ControlSessionError::NoPendingAuthentication)?;
        if !matches!(pending, PendingProof::Authentication) {
            return Err(ControlSessionError::NoPendingAuthentication);
        }
        let proof = self
            .proof_session
            .verify(response, OffsetDateTime::now_utc())?;
        let endpoint = self
            .control
            .authenticate_registered_endpoint(proof)
            .await
            .map_err(ControlSessionError::endpoint_authentication)?;
        let result = EndpointAuthenticationResult {
            tenant_id: endpoint.tenant_id,
            endpoint_key: endpoint.endpoint_key,
            principal: endpoint.principal,
        };
        self.endpoint = Some(endpoint);
        Ok(result)
    }
}

impl ControlSessionError {
    fn endpoint_authentication(error: ServiceError) -> Self {
        match error {
            ServiceError::Store(StoreError::NotFound)
            | ServiceError::WrongEndpointProofPurpose
            | ServiceError::WrongEndpointProofPrincipal
            | ServiceError::EndpointIdentityChanged => Self::EndpointAuthenticationFailed,
            other => Self::Service(other),
        }
    }
}

fn authenticated_response(request_id: RequestId, account: Account) -> ControlServerMessage {
    ControlServerMessage::AccountAuthenticated {
        request_id,
        user_id: account.id,
        username: account.username,
        personal_tenant_id: account.personal_tenant_id,
    }
}

fn error_response(
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
        ControlSessionError::EndpointAuthenticationFailed => (
            ControlErrorCode::InvalidCredentials,
            "endpoint authentication failed".to_owned(),
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
enum ControlSessionError {
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
    #[error(transparent)]
    EndpointProof(#[from] EndpointProofError),
    #[error(transparent)]
    Service(#[from] ServiceError),
}
