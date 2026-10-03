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

use crate::{
    active_endpoints::ActiveEndpoints,
    auth::{CredentialError, PasswordEngine, PasswordPolicy, normalize_username},
    domain::{Account, Device, RegisteredEndpoint, Team, TeamRole},
    endpoint_proof::VerifiedEndpointProof,
    postgres::{PostgresStore, StoreError},
};

#[derive(Debug, Clone)]
pub struct ControlPlane {
    store: PostgresStore,
    passwords: PasswordEngine,
    dummy_password_hash: String,
    active_endpoints: Arc<ActiveEndpoints>,
    web_revision: tokio::sync::watch::Sender<u64>,
}

impl ControlPlane {
    pub(crate) fn web_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.web_revision.subscribe()
    }

    pub(crate) fn web_changed(&self) {
        self.web_revision
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    pub async fn touch_online_devices(&self) -> Result<(), StoreError> {
        sqlx::query("UPDATE devices d SET last_online_at=clock_timestamp() WHERE EXISTS(SELECT 1 FROM endpoints e WHERE e.device_id=d.id AND e.status='active' AND e.endpoint_key=ANY($1::bytea[]))")
            .bind(self.online_endpoint_keys()).execute(self.store.pool()).await?;
        Ok(())
    }
    pub(crate) fn online_endpoint_keys(&self) -> Vec<Vec<u8>> {
        self.active_endpoints.keys()
    }
    pub async fn list_traffic_scopes(
        &self,
        account: &Account,
    ) -> Result<pab_protocol::TrafficScopeOptions, ServiceError> {
        Ok(self
            .store
            .list_traffic_scopes(account.id, account.personal_tenant_id)
            .await?)
    }

    pub async fn list_my_devices(
        &self,
        endpoint: &RegisteredEndpoint,
        deployment_id: DeploymentId,
    ) -> Result<Vec<pab_protocol::DeviceDirectoryEntry>, ServiceError> {
        let EndpointProofPrincipal::User { user_id } = endpoint.principal else {
            return Err(ServiceError::UserEndpointRequired);
        };
        Ok(self
            .store
            .list_my_devices(
                endpoint.endpoint_key,
                user_id,
                endpoint.tenant_id,
                deployment_id,
            )
            .await?)
    }

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
            web_revision: tokio::sync::watch::channel(0).0,
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

    pub async fn account_id_by_username(&self, username: &str) -> Result<UserId, ServiceError> {
        let (_, username_key) = normalize_username(username)?;
        self.store
            .account_credential(&username_key)
            .await?
            .map(|credential| credential.account.id)
            .ok_or(StoreError::NotFound.into())
    }

    pub async fn admin_create_team(
        &self,
        owner: UserId,
        name: &str,
        operator_label: &str,
    ) -> Result<Team, ServiceError> {
        Ok(self
            .store
            .admin_create_team(owner, name, operator_label)
            .await?)
    }

    pub async fn admin_add_team_member(
        &self,
        tenant_id: TenantId,
        user_id: UserId,
        role: TeamRole,
        operator_label: &str,
    ) -> Result<bool, ServiceError> {
        Ok(self
            .store
            .admin_add_team_member(tenant_id, user_id, role, operator_label)
            .await?)
    }

    pub async fn admin_remove_team_member(
        &self,
        tenant_id: TenantId,
        user_id: UserId,
        operator_label: &str,
    ) -> Result<bool, ServiceError> {
        Ok(self
            .store
            .admin_remove_team_member(tenant_id, user_id, operator_label)
            .await?)
    }

    pub async fn admin_set_default_traffic_team(
        &self,
        user_id: UserId,
        team_id: Option<TenantId>,
        operator_label: &str,
    ) -> Result<bool, ServiceError> {
        Ok(self
            .store
            .admin_set_default_traffic_team(user_id, team_id, operator_label)
            .await?)
    }

    pub async fn admin_set_team_limits(
        &self,
        tenant_id: TenantId,
        total_mbps: u32,
        member_mbps: u32,
        operator_label: &str,
    ) -> Result<bool, ServiceError> {
        Ok(self
            .store
            .admin_set_team_limits(tenant_id, total_mbps, member_mbps, operator_label)
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
        if self.active_endpoints.connected(endpoint_key) {
            self.web_changed();
            self.touch_device_endpoint(endpoint_key);
        }
    }

    pub(crate) fn endpoint_disconnected(&self, endpoint_key: EndpointKey) {
        if self.active_endpoints.disconnected(endpoint_key) {
            self.web_changed();
            self.touch_device_endpoint(endpoint_key);
        }
    }

    fn touch_device_endpoint(&self, endpoint_key: EndpointKey) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let pool = self.store.pool().clone();
            runtime.spawn(async move {
                if sqlx::query("UPDATE devices d SET last_online_at=clock_timestamp() WHERE EXISTS(SELECT 1 FROM endpoints e WHERE e.device_id=d.id AND e.endpoint_key=$1)")
                    .bind(endpoint_key.as_bytes().as_slice()).execute(&pool).await.is_err() {
                    tracing::warn!("could not save device last-online time");
                }
            });
        }
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
        let peer = self.store.registered_endpoint(peer_endpoint_key).await?;
        let authorized = match peer.principal {
            EndpointProofPrincipal::Guest => {
                self.store
                    .authorize_guest_device_peer(endpoint, device_id, peer_endpoint_key)
                    .await?
            }
            EndpointProofPrincipal::User { .. } => {
                self.store
                    .authorize_device_peer(endpoint, device_id, peer_endpoint_key)
                    .await?
            }
            EndpointProofPrincipal::Device { .. } => {
                return Err(ServiceError::UserEndpointRequired);
            }
        };
        self.store
            .renew_connection_intent(peer_endpoint_key, device_id)
            .await?;
        Ok(authorized)
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
        let snapshot = match endpoint.principal {
            EndpointProofPrincipal::User { user_id } => {
                self.store
                    .device_network_snapshot(
                        endpoint.endpoint_key,
                        user_id,
                        endpoint.tenant_id,
                        device_ref,
                    )
                    .await?
            }
            EndpointProofPrincipal::Guest => {
                self.store
                    .guest_device_network_snapshot(endpoint.endpoint_key, device_ref)
                    .await?
            }
            EndpointProofPrincipal::Device { .. } => {
                return Err(ServiceError::UserEndpointRequired);
            }
        };
        if !self.active_endpoints.is_connected(snapshot.endpoint_key) {
            return Err(ServiceError::DeviceOffline);
        }
        Ok(snapshot)
    }

    pub async fn resolve_device_code(
        &self,
        endpoint: &RegisteredEndpoint,
        code: pab_protocol::DeviceCode,
        deployment_id: pab_protocol::DeploymentId,
    ) -> Result<DeviceRef, ServiceError> {
        Ok(match endpoint.principal {
            EndpointProofPrincipal::User { user_id } => {
                self.store
                    .resolve_device_code(
                        endpoint.endpoint_key,
                        user_id,
                        endpoint.tenant_id,
                        deployment_id,
                        code,
                    )
                    .await?
            }
            EndpointProofPrincipal::Guest => {
                self.store
                    .guest_resolve_device_code(endpoint.endpoint_key, code, deployment_id)
                    .await?
            }
            EndpointProofPrincipal::Device { .. } => {
                return Err(ServiceError::UserEndpointRequired);
            }
        })
    }

    pub async fn device_presence(
        &self,
        endpoint: &RegisteredEndpoint,
        code: pab_protocol::DeviceCode,
    ) -> Result<pab_protocol::DevicePresence, ServiceError> {
        if matches!(endpoint.principal, EndpointProofPrincipal::Device { .. }) {
            return Err(ServiceError::UserEndpointRequired);
        }
        let (name, key) = self
            .store
            .device_presence(endpoint.endpoint_key, code)
            .await?;
        Ok(pab_protocol::DevicePresence {
            code,
            name,
            online: key.is_some_and(|key| self.active_endpoints.is_connected(key)),
        })
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
    #[error("the device is offline")]
    DeviceOffline,
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
