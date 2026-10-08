use std::path::Path;

use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
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
        // Legacy reset drops the entire schema, including related task tables.
        .foreign_keys(false)
        .busy_timeout(std::time::Duration::from_secs(10))
        .create_if_missing(create);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    reset_legacy_database(&pool, path).await?;
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

const LEGACY_SCHEMA: &str =
    "SELECT COUNT(*) FROM pragma_table_info('device_access') WHERE name = 'deployment_id'";

async fn reset_legacy_database(pool: &SqlitePool, path: &Path) -> Result<(), sqlx::Error> {
    if sqlx::query_scalar::<_, i64>(LEGACY_SCHEMA)
        .fetch_one(pool)
        .await?
        == 0
    {
        return Ok(());
    }
    // Delete the obsolete schema/data atomically instead of unlinking a database
    // which another process may have open. No backup or migration is performed.
    // Recheck under the write lock so concurrent opens only reset it once.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if sqlx::query_scalar::<_, i64>(LEGACY_SCHEMA)
        .fetch_one(&mut *tx)
        .await?
        != 0
    {
        let objects = sqlx::query(
            "SELECT type, name FROM sqlite_schema WHERE type IN ('view', 'table') \
             AND name NOT GLOB 'sqlite_*' ORDER BY type DESC",
        )
        .fetch_all(&mut *tx)
        .await?;
        for object in objects {
            let kind: String = object.try_get("type")?;
            let name: String = object.try_get("name")?;
            let identifier = name.replace('"', "\"\"");
            sqlx::query(&format!("DROP {kind} \"{identifier}\""))
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        tracing::warn!(database = %path.display(), "deleted obsolete database schema and records; device identity key files retained");
    } else {
        tx.commit().await?;
    }
    Ok(())
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) async fn create_legacy_database(path: &Path) -> SqlitePool {
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        sqlx::query("CREATE TABLE device_access (id INTEGER PRIMARY KEY, deployment_id TEXT NOT NULL, tenant_id TEXT NOT NULL, device_id TEXT NOT NULL, device_code TEXT NOT NULL, temporary_password TEXT NOT NULL, password_version INTEGER NOT NULL, password_hash TEXT NOT NULL)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO device_access VALUES (1, 'old-deployment', 'tenant', 'device', '123456789', 'test', 1, 'old-hash')")
            .execute(&pool).await.unwrap();
        sqlx::raw_sql("CREATE TABLE old_tasks (id INTEGER PRIMARY KEY REFERENCES device_access(id)); INSERT INTO old_tasks VALUES (1); CREATE VIEW old_view AS SELECT * FROM old_tasks;")
            .execute(&pool).await.unwrap();
        pool
    }

    fn replacement() -> DeviceAccess {
        DeviceAccess {
            tenant_id: "new-tenant".into(),
            device_id: "new-device".into(),
            device_code: "987654321".into(),
            temporary_password: "new-test".into(),
            password_version: 2,
            password_hash: "new-hash".into(),
        }
    }

    #[tokio::test]
    async fn legacy_database_is_reset_and_can_be_initialized_with_keys_retained() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("executor.sqlite3");
        let key = directory.path().join("device-endpoint.key");
        std::fs::write(&key, b"identity-key").unwrap();
        let pool = create_legacy_database(&path).await;
        assert!(matches!(load(&path).await, Err(DeviceAccessError::Missing)));
        save(&path, &replacement()).await.unwrap();
        assert_eq!(load(&path).await.unwrap().password_hash, "new-hash");
        let legacy: i64 = sqlx::query_scalar(LEGACY_SCHEMA)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(legacy, 0);
        let old_objects: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sqlite_schema WHERE name IN ('old_tasks', 'old_view')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(old_objects, 0);
        assert_eq!(std::fs::read(key).unwrap(), b"identity-key");
        let tasks = crate::task_store::TaskStore::open(&path).await.unwrap();
        drop(tasks);
        // Reopening a current database must preserve its tasks and password.
        sqlx::query("CREATE TABLE preserved (value TEXT); INSERT INTO preserved VALUES ('keep')")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(load(&path).await.unwrap().password_hash, "new-hash");
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT value FROM preserved")
                .fetch_one(&pool)
                .await
                .unwrap(),
            "keep"
        );
        pool.close().await;
    }

    #[tokio::test]
    async fn concurrent_opens_only_reset_legacy_data_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("executor.sqlite3");
        let pool = create_legacy_database(&path).await;
        let access = replacement();
        let (first, ()) = tokio::join!(save(&path, &access), async {
            let other = open(&path, true).await.unwrap();
            other.close().await;
        });
        first.unwrap();
        assert_eq!(load(&path).await.unwrap().password_hash, "new-hash");
        pool.close().await;
    }

    #[tokio::test]
    async fn corrupt_database_is_not_treated_as_a_legacy_schema() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("executor.sqlite3");
        let bytes = b"not a sqlite database";
        std::fs::write(&path, bytes).unwrap();
        assert!(load(&path).await.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
