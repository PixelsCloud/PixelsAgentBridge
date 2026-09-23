use std::time::Duration;

use pab_protocol::{
    DeploymentId, DeviceId, EndpointKey, EndpointProofPrincipal, RELAY_POLICY_SCHEMA_VERSION,
    RelayEndpointOwner, RelayEndpointPolicy, RelayLimitDefaults, RelayPolicySnapshot,
    TeamRelayLimits, TenantId, TrafficScope, UserId,
};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::domain::{
    Account, AccountCredential, Device, RegisteredEndpoint, Team, TeamInvitation, TeamRole,
};

mod accounts;
mod device_claim;
mod device_discovery;
mod device_grants;
mod device_network;
mod device_runtime;
mod endpoints;
mod guest_access;
mod guest_registration;
mod peer_authorization;
mod support;
mod teams;

#[derive(Debug, Clone)]
pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn connect(database_url: &str, max_connections: u32) -> Result<Self, StoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(database_url)
            .await?;
        Ok(Self::from_pool(pool))
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn migrate(&self) -> Result<(), StoreError> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    pub async fn initialize_deployment(
        &self,
        requested_id: DeploymentId,
        defaults: RelayLimitDefaults,
    ) -> Result<DeploymentId, StoreError> {
        defaults
            .validate()
            .map_err(|error| StoreError::InvalidInput(error.to_string()))?;
        let row = sqlx::query(
            r#"
            INSERT INTO deployments (
                id, default_team_mbps, default_member_mbps, default_personal_mbps
            )
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (singleton) DO UPDATE SET singleton = deployments.singleton
            RETURNING id
            "#,
        )
        .bind(requested_id.as_uuid())
        .bind(i32::try_from(defaults.team_mbps).map_err(support::invalid_number)?)
        .bind(i32::try_from(defaults.member_mbps).map_err(support::invalid_number)?)
        .bind(i32::try_from(defaults.personal_mbps).map_err(support::invalid_number)?)
        .fetch_one(&self.pool)
        .await?;
        Ok(DeploymentId::from_uuid(row.try_get("id")?))
    }

    pub async fn deployment_id(&self) -> Result<DeploymentId, StoreError> {
        let id = sqlx::query_scalar::<_, Uuid>("SELECT id FROM deployments WHERE singleton = true")
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::InvalidState("deployment is not initialized".to_owned()))?;
        Ok(DeploymentId::from_uuid(id))
    }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("resource was not found")]
    NotFound,
    #[error("operation is not permitted in this tenant")]
    PermissionDenied,
    #[error("resource conflict: {0}")]
    Conflict(&'static str),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("invalid state: {0}")]
    InvalidState(String),
    #[error("stored data violates the application contract: {0}")]
    InvalidData(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Migration(#[from] sqlx::migrate::MigrateError),
}
