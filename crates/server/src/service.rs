use std::{sync::Arc, time::Duration};

use pab_protocol::{
    AuthorizedDevicePeer, DEVICE_NETWORK_SCHEMA_VERSION, DEVICE_SESSION_SCHEMA_VERSION,
    DeploymentId, DeviceHello, DeviceHelloResult, DeviceNetworkResult, DeviceNetworkSnapshot,
    DeviceNetworkUpdate, DeviceRef, EndpointKey, EndpointProofPrincipal, EndpointProofPurpose,
    MAX_DEVICE_DIRECT_ADDRESSES, MAX_DEVICE_RELAY_URLS, RelayLimitDefaults, RelayPolicySnapshot,
    TenantId, UserId,
};
use pab_task_runtime::{TaskRuntimeError, validate_execution_context};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    active_endpoints::ActiveEndpoints,
    auth::{CredentialError, PasswordEngine, PasswordPolicy, normalize_username},
    domain::{Account, Device, RegisteredEndpoint, Team, TeamInvitation, TeamRole},
    endpoint_proof::VerifiedEndpointProof,
    postgres::{PostgresStore, StoreError},
};

#[derive(Debug, Clone)]
pub struct ControlPlane {
    store: PostgresStore,
    passwords: PasswordEngine,
    dummy_password_hash: String,
    active_endpoints: Arc<ActiveEndpoints>,
}

impl ControlPlane {
    pub fn new(
        store: PostgresStore,
        password_policy: PasswordPolicy,
    ) -> Result<Self, ServiceError> {
        let passwords = PasswordEngine::new(password_policy);
        let dummy_password_hash = passwords.hash("pab-dummy-credential-not-a-user")?;
        Ok(Self {
            store,
            passwords,
            dummy_password_hash,
            active_endpoints: Arc::new(ActiveEndpoints::default()),
        })
    }

    pub fn store(&self) -> &PostgresStore {
        &self.store
    }

    pub async fn initialize_deployment(
        &self,
        requested_id: DeploymentId,
        defaults: RelayLimitDefaults,
    ) -> Result<DeploymentId, ServiceError> {
        Ok(self
            .store
            .initialize_deployment(requested_id, defaults)
            .await?)
    }

    pub async fn register_account(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Account, ServiceError> {
        let (username, username_key) = normalize_username(username)?;
        let passwords = self.passwords.clone();
        let password = password.to_owned();
        let password_hash = tokio::task::spawn_blocking(move || passwords.hash(&password))
            .await
            .map_err(ServiceError::PasswordTask)??;
        Ok(self
            .store
            .register_account(&username, &username_key, &password_hash)
            .await?)
    }

    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Account, ServiceError> {
        let (_, username_key) =
            normalize_username(username).map_err(|_| ServiceError::InvalidCredentials)?;
        let credential = self.store.account_credential(&username_key).await?;
        let encoded_hash = credential
            .as_ref()
            .map(|credential| credential.password_hash.clone())
            .unwrap_or_else(|| self.dummy_password_hash.clone());
        let passwords = self.passwords.clone();
        let password = password.to_owned();
        let verified =
            tokio::task::spawn_blocking(move || passwords.verify(&password, &encoded_hash))
                .await
                .map_err(ServiceError::PasswordTask)?;
        match (verified, credential) {
            (true, Some(credential)) => Ok(credential.account),
            _ => Err(ServiceError::InvalidCredentials),
        }
    }

    pub async fn create_team(&self, actor: UserId, name: &str) -> Result<Team, ServiceError> {
        Ok(self.store.create_team(actor, name).await?)
    }

    pub async fn invite_team_member(
        &self,
        actor: UserId,
        tenant_id: TenantId,
        invited_user: UserId,
        role: TeamRole,
        expires_at: OffsetDateTime,
    ) -> Result<TeamInvitation, ServiceError> {
        Ok(self
            .store
            .invite_team_member(actor, tenant_id, invited_user, role, expires_at)
            .await?)
    }

    pub async fn accept_team_invitation(
        &self,
        actor: UserId,
        invitation_id: Uuid,
    ) -> Result<TenantId, ServiceError> {
        Ok(self
            .store
            .accept_team_invitation(actor, invitation_id)
            .await?)
    }

    pub async fn register_user_endpoint(
        &self,
        proof: VerifiedEndpointProof,
    ) -> Result<(), ServiceError> {
        if proof.purpose() != EndpointProofPurpose::RegisterUserEndpoint {
            return Err(ServiceError::WrongEndpointProofPurpose);
        }
        let EndpointProofPrincipal::User { user_id } = proof.principal() else {
            return Err(ServiceError::WrongEndpointProofPrincipal);
        };
        Ok(self
            .store
            .register_user_endpoint(user_id, proof.tenant_id(), proof.endpoint_key())
            .await?)
    }

    pub async fn register_device(
        &self,
        proof: VerifiedEndpointProof,
        name: &str,
    ) -> Result<Device, ServiceError> {
        if proof.purpose() != EndpointProofPurpose::RegisterDevice {
            return Err(ServiceError::WrongEndpointProofPurpose);
        }
        let EndpointProofPrincipal::User { user_id } = proof.principal() else {
            return Err(ServiceError::WrongEndpointProofPrincipal);
        };
        Ok(self
            .store
            .register_device(user_id, proof.tenant_id(), name, proof.endpoint_key())
            .await?)
    }

    pub async fn set_device_connect_grant(
        &self,
        actor: UserId,
        tenant_id: TenantId,
        device_id: pab_protocol::DeviceId,
        user_id: UserId,
        allowed: bool,
    ) -> Result<(), ServiceError> {
        Ok(self
            .store
            .set_device_connect_grant(actor, tenant_id, device_id, user_id, allowed)
            .await?)
    }

    pub async fn authenticate_registered_endpoint(
        &self,
        proof: VerifiedEndpointProof,
    ) -> Result<RegisteredEndpoint, ServiceError> {
        if proof.purpose() != EndpointProofPurpose::AuthenticateRegisteredEndpoint {
            return Err(ServiceError::WrongEndpointProofPurpose);
        }
        let endpoint = self.store.registered_endpoint(proof.endpoint_key()).await?;
        if endpoint.tenant_id != proof.tenant_id() || endpoint.principal != proof.principal() {
            return Err(ServiceError::EndpointIdentityChanged);
        }
        Ok(endpoint)
    }

    pub async fn register_unclaimed_device(
        &self,
        proof: VerifiedEndpointProof,
        name: &str,
    ) -> Result<Device, ServiceError> {
        if proof.purpose() != EndpointProofPurpose::RegisterUnclaimedDevice {
            return Err(ServiceError::WrongEndpointProofPurpose);
        }
        let EndpointProofPrincipal::Device { device_id } = proof.principal() else {
            return Err(ServiceError::WrongEndpointProofPrincipal);
        };
        Ok(self
            .store
            .register_unclaimed_device(proof.tenant_id(), device_id, name, proof.endpoint_key())
            .await?)
    }

    pub async fn register_guest_endpoint(
        &self,
        proof: VerifiedEndpointProof,
    ) -> Result<TenantId, ServiceError> {
        if proof.purpose() != EndpointProofPurpose::RegisterGuestEndpoint {
            return Err(ServiceError::WrongEndpointProofPurpose);
        }
        if proof.principal() != EndpointProofPrincipal::Guest {
            return Err(ServiceError::WrongEndpointProofPrincipal);
        }
        Ok(self
            .store
            .register_guest_endpoint(proof.tenant_id(), proof.endpoint_key())
            .await?)
    }

    pub async fn registered_endpoint(
        &self,
        endpoint_key: pab_protocol::EndpointKey,
    ) -> Result<RegisteredEndpoint, ServiceError> {
        Ok(self.store.registered_endpoint(endpoint_key).await?)
    }

    pub(crate) fn endpoint_connected(&self, endpoint_key: EndpointKey) {
        self.active_endpoints.connected(endpoint_key);
    }

    pub(crate) fn endpoint_disconnected(&self, endpoint_key: EndpointKey) {
        self.active_endpoints.disconnected(endpoint_key);
    }

    pub async fn authorize_device_peer(
        &self,
        endpoint: &RegisteredEndpoint,
        peer_endpoint_key: EndpointKey,
    ) -> Result<AuthorizedDevicePeer, ServiceError> {
        let EndpointProofPrincipal::Device { device_id } = endpoint.principal else {
            return Err(ServiceError::DeviceEndpointRequired);
        };
        if !self.active_endpoints.is_connected(peer_endpoint_key) {
            return Err(ServiceError::PeerEndpointOffline);
        }
        Ok(self
            .store
            .authorize_device_peer(endpoint, device_id, peer_endpoint_key)
            .await?)
    }

    pub async fn publish_device_hello(
        &self,
        endpoint: &RegisteredEndpoint,
        hello: &DeviceHello,
    ) -> Result<DeviceHelloResult, ServiceError> {
        if hello.schema_version != DEVICE_SESSION_SCHEMA_VERSION {
            return Err(ServiceError::UnsupportedDeviceSessionSchema(
                hello.schema_version,
            ));
        }
        let EndpointProofPrincipal::Device { device_id } = endpoint.principal else {
            return Err(ServiceError::DeviceEndpointRequired);
        };
        if hello.device_ref.tenant_id != endpoint.tenant_id
            || hello.device_ref.device_id != device_id
        {
            return Err(ServiceError::EndpointIdentityChanged);
        }
        let agent_version = hello.agent_version.trim();
        if agent_version.is_empty()
            || agent_version != hello.agent_version
            || agent_version.chars().count() > 64
        {
            return Err(ServiceError::InvalidAgentVersion);
        }
        if hello.observed_at_unix_ms <= 0 {
            return Err(ServiceError::InvalidObservedTime);
        }
        validate_execution_context(&hello.execution_context)?;
        Ok(self
            .store
            .publish_device_hello(endpoint.endpoint_key, hello)
            .await?)
    }

    pub async fn publish_device_network(
        &self,
        endpoint: &RegisteredEndpoint,
        update: &DeviceNetworkUpdate,
    ) -> Result<DeviceNetworkResult, ServiceError> {
        if update.schema_version != DEVICE_NETWORK_SCHEMA_VERSION {
            return Err(ServiceError::UnsupportedDeviceNetworkSchema(
                update.schema_version,
            ));
        }
        let EndpointProofPrincipal::Device { device_id } = endpoint.principal else {
            return Err(ServiceError::DeviceEndpointRequired);
        };
        if update.device_ref.tenant_id != endpoint.tenant_id
            || update.device_ref.device_id != device_id
            || update.endpoint_key != endpoint.endpoint_key
        {
            return Err(ServiceError::EndpointIdentityChanged);
        }
        if update.address_revision == 0 {
            return Err(ServiceError::InvalidAddressRevision);
        }
        if update.observed_at_unix_ms <= 0 {
            return Err(ServiceError::InvalidObservedTime);
        }
        if update.relay_urls.len() > MAX_DEVICE_RELAY_URLS
            || update.direct_addresses.len() > MAX_DEVICE_DIRECT_ADDRESSES
        {
            return Err(ServiceError::TooManyDeviceAddresses);
        }
        for relay_url in &update.relay_urls {
            let parsed =
                url::Url::parse(relay_url).map_err(|_| ServiceError::InvalidDeviceRelayUrl)?;
            if parsed.scheme() != "https" || parsed.host_str().is_none() || relay_url.len() > 2_048
            {
                return Err(ServiceError::InvalidDeviceRelayUrl);
            }
        }
        if update
            .direct_addresses
            .iter()
            .any(|address| address.port() == 0 || address.ip().is_unspecified())
        {
            return Err(ServiceError::InvalidDeviceDirectAddress);
        }
        Ok(self.store.publish_device_network(update).await?)
    }

    pub async fn device_network_snapshot(
        &self,
        endpoint: &RegisteredEndpoint,
        device_ref: DeviceRef,
    ) -> Result<DeviceNetworkSnapshot, ServiceError> {
        let EndpointProofPrincipal::User { user_id } = endpoint.principal else {
            return Err(ServiceError::UserEndpointRequired);
        };
        if device_ref.tenant_id != endpoint.tenant_id {
            return Err(ServiceError::EndpointIdentityChanged);
        }
        Ok(self
            .store
            .device_network_snapshot(endpoint.endpoint_key, user_id, device_ref)
            .await?)
    }

    pub async fn resolve_device_code(
        &self,
        endpoint: &RegisteredEndpoint,
        code: pab_protocol::DeviceCode,
        deployment_id: pab_protocol::DeploymentId,
    ) -> Result<DeviceRef, ServiceError> {
        let EndpointProofPrincipal::User { user_id } = endpoint.principal else {
            return Err(ServiceError::UserEndpointRequired);
        };
        Ok(self
            .store
            .resolve_device_code(
                endpoint.endpoint_key,
                user_id,
                endpoint.tenant_id,
                deployment_id,
                code,
            )
            .await?)
    }

    pub async fn relay_policy_snapshot(
        &self,
        validity: Duration,
    ) -> Result<RelayPolicySnapshot, ServiceError> {
        Ok(self.store.relay_policy_snapshot(validity).await?)
    }
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("invalid username or password")]
    InvalidCredentials,
    #[error("endpoint proof was issued for a different operation")]
    WrongEndpointProofPurpose,
    #[error("endpoint proof principal does not match the operation")]
    WrongEndpointProofPrincipal,
    #[error("registered endpoint identity changed while authentication was in progress")]
    EndpointIdentityChanged,
    #[error("device session schema version {0} is not supported")]
    UnsupportedDeviceSessionSchema(u16),
    #[error("device network schema version {0} is not supported")]
    UnsupportedDeviceNetworkSchema(u16),
    #[error("this operation requires an authenticated device endpoint")]
    DeviceEndpointRequired,
    #[error("this operation requires an authenticated user endpoint")]
    UserEndpointRequired,
    #[error("the peer endpoint does not have an active control connection")]
    PeerEndpointOffline,
    #[error("agent version must contain between 1 and 64 characters")]
    InvalidAgentVersion,
    #[error("device observation time must be a positive Unix timestamp")]
    InvalidObservedTime,
    #[error("device address revision must be greater than zero")]
    InvalidAddressRevision,
    #[error("device published too many network addresses")]
    TooManyDeviceAddresses,
    #[error("device Relay URLs must be absolute HTTPS URLs of at most 2048 bytes")]
    InvalidDeviceRelayUrl,
    #[error("device direct addresses must use a specific IP and nonzero port")]
    InvalidDeviceDirectAddress,
    #[error(transparent)]
    TaskRuntime(#[from] TaskRuntimeError),
    #[error(transparent)]
    Credential(#[from] CredentialError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("password worker failed: {0}")]
    PasswordTask(tokio::task::JoinError),
}
