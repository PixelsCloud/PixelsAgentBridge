use std::{collections::HashSet, time::Duration};

use pab_protocol::{
    ControlClientMessage, ControlServerMessage, DeploymentId, EndpointAuthenticationResult,
    EndpointKey, EndpointProofPrincipal, EndpointProofPurpose, EndpointRegistration,
    EndpointRegistrationResult, RequestId,
};
use time::OffsetDateTime;

use super::{
    ControlApiConfig,
    session_error::{ControlSessionError, error_response},
};
use crate::{
    ControlPlane, EndpointProofSession,
    domain::{Account, RegisteredEndpoint},
};

const ENDPOINT_CHALLENGE_VALIDITY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
enum PendingRegistration {
    User,
    Device { name: String },
    UnclaimedDevice { name: String },
    Guest,
}

#[derive(Debug, Clone)]
enum PendingProof {
    Registration(PendingRegistration),
    Authentication,
}

pub struct ControlSession {
    control: ControlPlane,
    deployment_id: DeploymentId,
    config: ControlApiConfig,
    proof_session: EndpointProofSession,
    account: Option<Account>,
    endpoint: Option<RegisteredEndpoint>,
    pending_proof: Option<PendingProof>,
    open_registration_attempts: u8,
    guest_code_lookups: HashSet<pab_protocol::DeviceCode>,
}

impl ControlSession {
    pub fn new(
        control: ControlPlane,
        deployment_id: DeploymentId,
        config: ControlApiConfig,
    ) -> Self {
        Self {
            control,
            deployment_id,
            config,
            proof_session: EndpointProofSession::new(deployment_id),
            account: None,
            endpoint: None,
            pending_proof: None,
            open_registration_attempts: 0,
            guest_code_lookups: HashSet::new(),
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
            ControlClientMessage::BeginUnclaimedDeviceRegistration {
                endpoint_key, name, ..
            } => self
                .begin_unclaimed_device_registration(endpoint_key, name)
                .map(|challenge| ControlServerMessage::EndpointChallenge {
                    request_id,
                    challenge,
                }),
            ControlClientMessage::BeginGuestEndpointRegistration { endpoint_key, .. } => self
                .begin_guest_registration(endpoint_key)
                .map(|challenge| ControlServerMessage::EndpointChallenge {
                    request_id,
                    challenge,
                }),
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
            ControlClientMessage::PublishDeviceHello { hello, .. } => self
                .publish_device_hello(&hello)
                .await
                .map(|result| ControlServerMessage::DeviceHelloAccepted { request_id, result }),
            ControlClientMessage::PublishDeviceNetwork { update, .. } => self
                .publish_device_network(&update)
                .await
                .map(|result| ControlServerMessage::DeviceNetworkAccepted { request_id, result }),
            ControlClientMessage::GetDeviceNetwork { device_ref, .. } => self
                .get_device_network(device_ref)
                .await
                .map(|snapshot| ControlServerMessage::DeviceNetworkFound {
                    request_id,
                    snapshot: Box::new(snapshot),
                }),
            ControlClientMessage::ResolveDeviceCode { device_code, .. } => self
                .resolve_device_code(device_code)
                .await
                .map(|device_ref| ControlServerMessage::DeviceCodeResolved {
                    request_id,
                    device_ref,
                }),
            ControlClientMessage::GetDevicePresence { device_code, .. } => self
                .get_device_presence(device_code)
                .await
                .map(|presence| ControlServerMessage::DevicePresenceFound {
                    request_id,
                    presence,
                }),
            ControlClientMessage::ListDevices { .. } => {
                self.list_devices()
                    .await
                    .map(|devices| ControlServerMessage::DeviceList {
                        request_id,
                        devices,
                    })
            }
            ControlClientMessage::ListTrafficScopes { .. } => {
                self.list_traffic_scopes().await.map(|options| {
                    ControlServerMessage::TrafficScopeList {
                        request_id,
                        options,
                    }
                })
            }
            ControlClientMessage::AuthorizeDevicePeer {
                peer_endpoint_key, ..
            } => self
                .authorize_device_peer(peer_endpoint_key)
                .await
                .map(|result| ControlServerMessage::DevicePeerAuthorized { request_id, result }),
            ControlClientMessage::BeginDeviceClaim {
                device_code,
                owner_tenant_id,
                ..
            } => self
                .begin_device_claim(device_code, owner_tenant_id)
                .await
                .map(|claim_id| ControlServerMessage::DeviceClaimPending {
                    request_id,
                    claim_id,
                }),
            ControlClientMessage::ApproveDeviceClaim { claim_id, .. } => self
                .approve_device_claim(claim_id)
                .await
                .map(
                    |(device_id, owner_tenant_id)| ControlServerMessage::DeviceClaimApproved {
                        request_id,
                        claim_id,
                        device_id,
                        owner_tenant_id,
                    },
                ),
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

    async fn list_traffic_scopes(
        &self,
    ) -> Result<pab_protocol::TrafficScopeOptions, ControlSessionError> {
        let account = self
            .account
            .as_ref()
            .ok_or(ControlSessionError::NotAuthenticated)?;
        self.control
            .list_traffic_scopes(account)
            .await
            .map_err(Into::into)
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

    fn begin_unclaimed_device_registration(
        &mut self,
        endpoint_key: EndpointKey,
        name: String,
    ) -> Result<pab_protocol::EndpointProofChallenge, ControlSessionError> {
        if self.is_authenticated() {
            return Err(ControlSessionError::AlreadyAuthenticated);
        }
        if self.pending_proof.is_some() {
            return Err(ControlSessionError::ProofPending);
        }
        if self.open_registration_attempts >= 3 {
            return Err(ControlSessionError::RateLimited);
        }
        self.open_registration_attempts += 1;
        let challenge = self.proof_session.issue(
            EndpointProofPrincipal::Device {
                device_id: pab_protocol::DeviceId::new(),
            },
            pab_protocol::TenantId::new(),
            endpoint_key,
            EndpointProofPurpose::RegisterUnclaimedDevice,
            OffsetDateTime::now_utc(),
            ENDPOINT_CHALLENGE_VALIDITY,
        )?;
        self.pending_proof = Some(PendingProof::Registration(
            PendingRegistration::UnclaimedDevice { name },
        ));
        Ok(challenge)
    }

    fn begin_guest_registration(
        &mut self,
        endpoint_key: EndpointKey,
    ) -> Result<pab_protocol::EndpointProofChallenge, ControlSessionError> {
        if self.is_authenticated() {
            return Err(ControlSessionError::AlreadyAuthenticated);
        }
        if self.pending_proof.is_some() {
            return Err(ControlSessionError::ProofPending);
        }
        if self.open_registration_attempts >= 3 {
            return Err(ControlSessionError::RateLimited);
        }
        self.open_registration_attempts += 1;
        let challenge = self.proof_session.issue(
            EndpointProofPrincipal::Guest,
            pab_protocol::TenantId::new(),
            endpoint_key,
            EndpointProofPurpose::RegisterGuestEndpoint,
            OffsetDateTime::now_utc(),
            ENDPOINT_CHALLENGE_VALIDITY,
        )?;
        self.pending_proof = Some(PendingProof::Registration(PendingRegistration::Guest));
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
                    device_code: device.code,
                    endpoint_key,
                })
            }
            PendingRegistration::UnclaimedDevice { name } => {
                let device = self.control.register_unclaimed_device(proof, &name).await?;
                Ok(EndpointRegistrationResult::Device {
                    tenant_id: device.tenant_id,
                    device_id: device.id,
                    device_code: device.code,
                    endpoint_key,
                })
            }
            PendingRegistration::Guest => {
                let tenant_id = self.control.register_guest_endpoint(proof).await?;
                Ok(EndpointRegistrationResult::Guest {
                    tenant_id,
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
        self.control.endpoint_connected(endpoint.endpoint_key);
        self.endpoint = Some(endpoint);
        Ok(result)
    }

    async fn publish_device_hello(
        &self,
        hello: &pab_protocol::DeviceHello,
    ) -> Result<pab_protocol::DeviceHelloResult, ControlSessionError> {
        let endpoint = self
            .endpoint
            .as_ref()
            .ok_or(ControlSessionError::DeviceEndpointRequired)?;
        if hello.device_ref.deployment_id != self.deployment_id {
            return Err(ControlSessionError::DeviceIdentityMismatch);
        }
        self.control
            .publish_device_hello(endpoint, hello)
            .await
            .map_err(Into::into)
    }

    async fn publish_device_network(
        &self,
        update: &pab_protocol::DeviceNetworkUpdate,
    ) -> Result<pab_protocol::DeviceNetworkResult, ControlSessionError> {
        let endpoint = self
            .endpoint
            .as_ref()
            .ok_or(ControlSessionError::DeviceEndpointRequired)?;
        if update.device_ref.deployment_id != self.deployment_id {
            return Err(ControlSessionError::DeviceIdentityMismatch);
        }
        self.control
            .publish_device_network(endpoint, update)
            .await
            .map_err(Into::into)
    }

    async fn get_device_network(
        &self,
        device_ref: pab_protocol::DeviceRef,
    ) -> Result<pab_protocol::DeviceNetworkSnapshot, ControlSessionError> {
        let endpoint = self
            .endpoint
            .as_ref()
            .ok_or(ControlSessionError::UserEndpointRequired)?;
        if device_ref.deployment_id != self.deployment_id {
            return Err(ControlSessionError::DeviceIdentityMismatch);
        }
        self.control
            .device_network_snapshot(endpoint, device_ref)
            .await
            .map_err(Into::into)
    }

    async fn resolve_device_code(
        &mut self,
        device_code: pab_protocol::DeviceCode,
    ) -> Result<pab_protocol::DeviceRef, ControlSessionError> {
        self.accept_code_lookup(device_code)?;
        let endpoint = self
            .endpoint
            .as_ref()
            .ok_or(ControlSessionError::UserEndpointRequired)?;
        self.control
            .resolve_device_code(endpoint, device_code, self.deployment_id)
            .await
            .map_err(Into::into)
    }

    async fn get_device_presence(
        &mut self,
        device_code: pab_protocol::DeviceCode,
    ) -> Result<pab_protocol::DevicePresence, ControlSessionError> {
        self.accept_code_lookup(device_code)?;
        let endpoint = self
            .endpoint
            .as_ref()
            .ok_or(ControlSessionError::UserEndpointRequired)?;
        self.control
            .device_presence(endpoint, device_code)
            .await
            .map_err(Into::into)
    }

    fn accept_code_lookup(
        &mut self,
        device_code: pab_protocol::DeviceCode,
    ) -> Result<(), ControlSessionError> {
        if !self.guest_code_lookups.contains(&device_code) && self.guest_code_lookups.len() >= 20 {
            return Err(ControlSessionError::RateLimited);
        }
        self.guest_code_lookups.insert(device_code);
        Ok(())
    }

    async fn list_devices(
        &self,
    ) -> Result<Vec<pab_protocol::DeviceDirectoryEntry>, ControlSessionError> {
        let endpoint = self
            .endpoint
            .as_ref()
            .ok_or(ControlSessionError::UserEndpointRequired)?;
        self.control
            .list_my_devices(endpoint, self.deployment_id)
            .await
            .map_err(Into::into)
    }

    async fn authorize_device_peer(
        &self,
        peer_endpoint_key: EndpointKey,
    ) -> Result<pab_protocol::AuthorizedDevicePeer, ControlSessionError> {
        let endpoint = self
            .endpoint
            .as_ref()
            .ok_or(ControlSessionError::DeviceEndpointRequired)?;
        self.control
            .authorize_device_peer(endpoint, peer_endpoint_key)
            .await
            .map_err(Into::into)
    }

    async fn begin_device_claim(
        &self,
        device_code: pab_protocol::DeviceCode,
        owner_tenant_id: pab_protocol::TenantId,
    ) -> Result<pab_protocol::ClaimId, ControlSessionError> {
        let account = self
            .account
            .as_ref()
            .ok_or(ControlSessionError::NotAuthenticated)?;
        self.control
            .begin_device_claim(account.id, device_code, owner_tenant_id)
            .await
            .map_err(Into::into)
    }

    async fn approve_device_claim(
        &self,
        claim_id: pab_protocol::ClaimId,
    ) -> Result<(pab_protocol::DeviceId, pab_protocol::TenantId), ControlSessionError> {
        let endpoint = self
            .endpoint
            .as_ref()
            .ok_or(ControlSessionError::DeviceEndpointRequired)?;
        self.control
            .approve_device_claim(endpoint, claim_id)
            .await
            .map_err(Into::into)
    }
}

impl Drop for ControlSession {
    fn drop(&mut self) {
        if let Some(endpoint) = self.endpoint.as_ref() {
            self.control.endpoint_disconnected(endpoint.endpoint_key);
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
