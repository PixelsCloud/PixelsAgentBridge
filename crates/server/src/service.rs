use std::{sync::Arc, time::Duration};

use pab_protocol::{
    AuthorizedDevicePeer, DEVICE_NETWORK_SCHEMA_VERSION, DEVICE_SESSION_SCHEMA_VERSION,
    DeviceHello, DeviceHelloResult, DeviceNetworkResult, DeviceNetworkSnapshot,
    DeviceNetworkUpdate, DeviceRef, EndpointKey, EndpointProofPrincipal, EndpointProofPurpose,
    MAX_DEVICE_DIRECT_ADDRESSES, MAX_DEVICE_RELAY_URLS, RelayPolicySnapshot, TenantId, UserId,
};
use pab_task_runtime::{TaskRuntimeError, validate_execution_context};
use thiserror::Error;

use crate::{
    active_endpoints::ActiveEndpoints,
    auth::{CredentialError, PasswordEngine, PasswordPolicy, normalize_username},
    domain::{Account, Device, RegisteredEndpoint},
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
    relay_cluster:
        tokio::sync::watch::Sender<Option<Arc<crate::relay_cluster::RelayClusterConfig>>>,
    relay_config_source: Option<(Arc<std::path::PathBuf>, [u8; 32])>,
    relay_reload_lock: Arc<tokio::sync::Mutex<()>>,
    pub(crate) relay_budget_clock: Arc<std::sync::Mutex<crate::relay_budget::BudgetClock>>,
    pub(crate) relay_probes: Arc<std::sync::Mutex<crate::relay_probe::RelayProbes>>,
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
    ) -> Result<Vec<pab_protocol::DeviceDirectoryEntry>, ServiceError> {
        self.require_operator_login(endpoint.endpoint_key).await?;
        let EndpointProofPrincipal::User { user_id } = endpoint.principal else {
            return Err(ServiceError::UserEndpointRequired);
        };
        Ok(self
            .store
            .list_my_devices(endpoint.endpoint_key, user_id, endpoint.tenant_id)
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
            relay_cluster: tokio::sync::watch::channel(None).0,
            relay_config_source: None,
            relay_reload_lock: Arc::new(tokio::sync::Mutex::new(())),
            relay_budget_clock: Arc::new(std::sync::Mutex::new(
                crate::relay_budget::BudgetClock::new(std::time::Instant::now()),
            )),
            relay_probes: Arc::new(std::sync::Mutex::new(
                crate::relay_probe::RelayProbes::default(),
            )),
        })
    }

    pub fn store(&self) -> &PostgresStore {
        &self.store
    }

    pub(crate) fn relay_config(&self) -> Option<Arc<crate::relay_cluster::RelayClusterConfig>> {
        self.relay_cluster.borrow().clone()
    }

    pub fn with_relay_config_source(
        mut self,
        path: std::path::PathBuf,
        legacy_secret: &str,
    ) -> Self {
        self.relay_config_source = Some((
            Arc::new(path),
            *blake3::hash(legacy_secret.as_bytes()).as_bytes(),
        ));
        self
    }

    pub async fn reload_relay_config(&self) -> Result<(), StoreError> {
        self.reload_relay_config_audited(None).await
    }

    pub(crate) async fn reload_relay_config_audited(
        &self,
        audit: Option<crate::audit::AuditRecord>,
    ) -> Result<(), StoreError> {
        let _guard = self.relay_reload_lock.lock().await;
        let (path, secret) = self.relay_config_source.as_ref().ok_or_else(|| {
            StoreError::InvalidState("configuration reload is not configured".into())
        })?;
        let path = path.clone();
        let new = tokio::task::spawn_blocking(move || crate::config::ServerConfig::load(&path))
            .await
            .map_err(|_| StoreError::InvalidState("configuration reload worker failed".into()))?
            .map_err(|_| {
                StoreError::InvalidInput(
                    "configuration validation failed; previous directory retained".into(),
                )
            })?;
        if *blake3::hash(new.relay.control_secret.as_bytes()).as_bytes() != *secret {
            return Err(StoreError::InvalidState(
                "changing the legacy control credential requires a service restart".into(),
            ));
        }
        let directory = new.relay.resolved_directory(&self.store).await?;
        if directory.is_some() != self.relay_config().is_some() {
            return Err(StoreError::InvalidState(
                "initial node directory activation requires a coordinated service restart".into(),
            ));
        }
        if let Some(mut config) = directory {
            self.store.apply_relay_capacity_overrides(&mut config).await?;
            self.store
                .configure_relay_cluster_audited(&config, audit)
                .await?;
            self.relay_cluster.send_replace(Some(Arc::new(config)));
            self.web_changed();
        } else if let Some(mut record) = audit {
            let mut tx = self.store.pool().begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock(26035001)")
                .execute(&mut *tx)
                .await?;
            let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND status='active' AND server_admin)").bind(record.actor).fetch_one(&mut *tx).await?;
            if !allowed {
                return Err(StoreError::PermissionDenied);
            }
            record.before = Some(crate::audit::AuditValues {
                nodes: Some(Vec::new()),
                ..Default::default()
            });
            record.after = record.before.clone();
            self.store.insert_audit(&mut tx, record).await?;
            tx.commit().await?;
        }
        Ok(())
    }

    pub async fn with_relay_cluster(
        self,
        config: Option<crate::relay_cluster::RelayClusterConfig>,
    ) -> Result<Self, StoreError> {
        if let Some(mut config) = config {
            self.store.apply_relay_capacity_overrides(&mut config).await?;
            config
                .validate()
                .map_err(|e| StoreError::InvalidInput(e.into()))?;
            self.store.configure_relay_cluster(&config).await?;
            self.relay_cluster.send_replace(Some(Arc::new(config)));
        } else {
            self.store.disable_relay_cluster().await?;
        }
        Ok(self)
    }

    pub(crate) async fn set_relay_capacity(
        &self, node: &str, capacity: u32, machine_ip: Option<String>, actor: uuid::Uuid, request_id: uuid::Uuid,
    ) -> Result<(), StoreError> {
        let _guard = self.relay_reload_lock.lock().await;
        let mut config = self.relay_config().ok_or_else(|| StoreError::InvalidState("Relay directory is not enabled".into()))?.as_ref().clone();
        let entry = config.nodes.iter_mut().find(|entry| entry.node_id == node).ok_or(StoreError::NotFound)?;
        entry.capacity_mbps = capacity;
        self.store.set_relay_capacity_audited(node, capacity, machine_ip, actor, request_id).await?;
        self.relay_cluster.send_replace(Some(Arc::new(config)));
        self.web_changed();
        Ok(())
    }

    pub async fn initialize_settings(&self) -> Result<(), ServiceError> {
        Ok(self.store.initialize_settings().await?)
    }

    pub async fn register_account(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Account, ServiceError> {
        self.register_account_with_policy(username, password, None)
            .await
    }

    // Trusted administrative callers retain their existing explicit creation
    // path. Every public self-registration supplies its deployment default.
    pub(crate) async fn register_account_with_policy(
        &self,
        username: &str,
        password: &str,
        registration_default: Option<bool>,
    ) -> Result<Account, ServiceError> {
        let (username, username_key) = normalize_username(username)?;
        let passwords = self.passwords.clone();
        let password = password.to_owned();
        let password_hash = tokio::task::spawn_blocking(move || passwords.hash(&password))
            .await
            .map_err(ServiceError::PasswordTask)??;
        Ok(self
            .store
            .register_account(
                &username,
                &username_key,
                &password_hash,
                registration_default,
            )
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
            .and_then(|credential| credential.password_hash.clone())
            .unwrap_or_else(|| self.dummy_password_hash.clone());
        let passwords = self.passwords.clone();
        let password = password.to_owned();
        let verified =
            tokio::task::spawn_blocking(move || passwords.verify(&password, &encoded_hash))
                .await
                .map_err(ServiceError::PasswordTask)?;
        match (verified, credential) {
            (true, Some(credential)) if credential.password_hash.is_some() => {
                Ok(credential.account)
            }
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
        if authorized.user_context.user.is_none() {
            return Err(ServiceError::LoginRequired);
        }
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
        self.require_operator_login(endpoint.endpoint_key).await?;
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
    ) -> Result<DeviceRef, ServiceError> {
        self.require_operator_login(endpoint.endpoint_key).await?;
        Ok(match endpoint.principal {
            EndpointProofPrincipal::User { user_id } => {
                self.store
                    .resolve_device_code(endpoint.endpoint_key, user_id, endpoint.tenant_id, code)
                    .await?
            }
            EndpointProofPrincipal::Guest => {
                self.store
                    .guest_resolve_device_code(endpoint.endpoint_key, code)
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
        self.require_operator_login(endpoint.endpoint_key).await?;
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

    async fn require_operator_login(&self, key: EndpointKey) -> Result<(), ServiceError> {
        if self.store.endpoint_user_context(key).await?.user.is_none() {
            return Err(ServiceError::LoginRequired);
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("login required: register or sign in before adding or controlling devices")]
    LoginRequired,
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
