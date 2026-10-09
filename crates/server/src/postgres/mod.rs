use std::time::Duration;

use pab_protocol::{
    DeviceId, EndpointKey, EndpointProofPrincipal, RELAY_POLICY_SCHEMA_VERSION, RelayEndpointOwner,
    RelayEndpointPolicy, RelayLimitDefaults, RelayPolicySnapshot, TenantId, TrafficScope, UserId,
};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use thiserror::Error;
use time::OffsetDateTime;

use crate::domain::{Account, AccountCredential, Device, RegisteredEndpoint};

mod accounts;
mod connection_intents;
mod device_discovery;
mod device_network;
mod device_presence;
mod device_runtime;
mod endpoints;
mod guest_access;
mod guest_registration;
mod peer_authorization;
mod saved_devices;
mod support;
mod traffic;
mod usage;

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

    pub async fn initialize_settings(
        &self,
        defaults: RelayLimitDefaults,
    ) -> Result<(), StoreError> {
        defaults
            .validate()
            .map_err(|error| StoreError::InvalidInput(error.to_string()))?;
        sqlx::query("INSERT INTO server_settings (default_user_mbps, default_guest_mbps) VALUES ($1, $2) ON CONFLICT (singleton) DO NOTHING")
            .bind(i32::try_from(defaults.user_mbps).map_err(support::invalid_number)?)
            .bind(i32::try_from(defaults.guest_mbps).map_err(support::invalid_number)?)
            .execute(&self.pool).await?;
        Ok(())
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
