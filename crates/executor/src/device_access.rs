use std::path::Path;

use sqlx::{Row, SqlitePool, sqlite::SqliteConnectOptions};
use thiserror::Error;

#[derive(Clone, Debug)]
pub(crate) struct DeviceAccess {
    pub tenant_id: String,
    pub device_id: String,
    pub device_code: String,
    pub temporary_password: String,
    pub password_version: u64,
    pub password_hash: String,
}

pub(crate) async fn load(path: &Path) -> Result<DeviceAccess, DeviceAccessError> {
    let pool = open(path, false).await?;
    let row = sqlx::query(
        "SELECT tenant_id, device_id, device_code, temporary_password, \
         password_version, password_hash FROM device_access WHERE id = 1",
    )
    .fetch_optional(&pool)
    .await
    .map_err(|error| match &error {
        sqlx::Error::Database(database)
            if database.message().contains("no such table: device_access") =>
        {
            DeviceAccessError::Missing
        }
        _ => DeviceAccessError::Sql(error),
    })?
    .ok_or(DeviceAccessError::Missing)?;
    Ok(DeviceAccess {
        tenant_id: row.try_get("tenant_id")?,
        device_id: row.try_get("device_id")?,
        device_code: row.try_get("device_code")?,
        temporary_password: row.try_get("temporary_password")?,
        password_version: row.try_get::<i64, _>("password_version")? as u64,
        password_hash: row.try_get("password_hash")?,
    })
}

pub(crate) async fn save(path: &Path, access: &DeviceAccess) -> Result<(), DeviceAccessError> {
    let pool = open(path, true).await?;
    sqlx::query(
        "INSERT INTO device_access (id,tenant_id, device_id, device_code, \
         temporary_password, password_version, password_hash) VALUES (1, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
         tenant_id = excluded.tenant_id, device_id = excluded.device_id, \
         device_code = excluded.device_code, temporary_password = excluded.temporary_password, \
         password_version = excluded.password_version, password_hash = excluded.password_hash",
    )
    .bind(&access.tenant_id)
    .bind(&access.device_id)
    .bind(&access.device_code)
    .bind(&access.temporary_password)
    .bind(access.password_version as i64)
    .bind(&access.password_hash)
    .execute(&pool)
    .await?;
    Ok(())
}

async fn open(path: &Path, create: bool) -> Result<SqlitePool, DeviceAccessError> {
    if create {
        pab_agent_core::ensure_data_parent(path)?;
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .busy_timeout(std::time::Duration::from_secs(10))
        .create_if_missing(create);
    let pool = SqlitePool::connect_with(options).await?;
    if create {
        pab_agent_core::restrict_private_file(path)?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS device_access (\
                id INTEGER PRIMARY KEY CHECK (id = 1), \
                tenant_id TEXT NOT NULL, \
                device_id TEXT NOT NULL, device_code TEXT NOT NULL, \
                temporary_password TEXT NOT NULL, password_version INTEGER NOT NULL, \
                password_hash TEXT NOT NULL)",
        )
        .execute(&pool)
        .await?;
    }
    Ok(pool)
}

#[derive(Debug, Error)]
pub enum DeviceAccessError {
    #[error("device access has not been initialized")]
    Missing,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sql(#[from] sqlx::Error),
}
