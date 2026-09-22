use std::time::Duration;

use pab_protocol::{
    DeploymentId, EndpointProofPurpose, RelayLimitDefaults, RelayPolicySnapshot, TenantId, UserId,
};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    auth::{CredentialError, PasswordEngine, PasswordPolicy, normalize_username},
    domain::{Account, Device, Team, TeamInvitation, TeamRole},
    endpoint_proof::VerifiedEndpointProof,
    postgres::{PostgresStore, StoreError},
};

#[derive(Debug, Clone)]
pub struct ControlPlane {
    store: PostgresStore,
    passwords: PasswordEngine,
    dummy_password_hash: String,
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
        Ok(self
            .store
            .register_user_endpoint(proof.user_id(), proof.tenant_id(), proof.endpoint_key())
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
        Ok(self
            .store
            .register_device(
                proof.user_id(),
                proof.tenant_id(),
                name,
                proof.endpoint_key(),
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
    #[error(transparent)]
    Credential(#[from] CredentialError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("password worker failed: {0}")]
    PasswordTask(tokio::task::JoinError),
}
