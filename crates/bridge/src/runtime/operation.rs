use std::time::{SystemTime, UNIX_EPOCH};

use pab_protocol::{DeviceRef, RequestId};
use sqlx::Row;

use super::store::{RuntimeStore, RuntimeStoreError};

#[derive(Debug, Clone)]
pub struct OperationRecord {
    pub id: String,
    pub device_ref: DeviceRef,
    pub initiated_by: String,
    pub kind: String,
    pub direction: String,
    pub source: String,
    pub destination: String,
    pub overwrite: bool,
    pub state: String,
    pub offset: u64,
    pub size: u64,
    pub started_at_unix_ms: i64,
    pub finished_at_unix_ms: Option<i64>,
    pub message: Option<String>,
}

impl RuntimeStore {
    pub async fn start_operation(
        &self,
        id: RequestId,
        device_ref: DeviceRef,
        initiated_by: &str,
        direction: &str,
        source: &str,
        destination: &str,
        overwrite: bool,
    ) -> Result<(), RuntimeStoreError> {
        sqlx::query(
            "INSERT INTO runtime_operations (id, device_ref_json, initiated_by, kind, direction, source, destination, overwrite, state, started_at_unix_ms) VALUES (?, ?, ?, 'file_transfer', ?, ?, ?, ?, 'running', ?)",
        )
        .bind(id.to_string())
        .bind(serde_json::to_string(&device_ref)?)
        .bind(initiated_by)
        .bind(direction)
        .bind(source)
        .bind(destination)
        .bind(overwrite)
        .bind(now_unix_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn operation_progress(
        &self,
        id: RequestId,
        offset: u64,
        size: u64,
    ) -> Result<(), RuntimeStoreError> {
        sqlx::query(
            "UPDATE runtime_operations SET offset = MAX(offset, ?), size = MAX(size, ?) WHERE id = ? AND state = 'running'",
        )
        .bind(i64::try_from(offset).map_err(|_| RuntimeStoreError::OffsetTooLarge)?)
        .bind(i64::try_from(size).map_err(|_| RuntimeStoreError::OffsetTooLarge)?)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_operation(
        &self,
        id: RequestId,
        state: &str,
        message: Option<&str>,
    ) -> Result<(), RuntimeStoreError> {
        sqlx::query(
            "UPDATE runtime_operations SET state = ?, message = ?, finished_at_unix_ms = ? WHERE id = ? AND state = 'running'",
        )
        .bind(state)
        .bind(message)
        .bind(now_unix_ms())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn operations(&self) -> Result<Vec<OperationRecord>, RuntimeStoreError> {
        let rows = sqlx::query("SELECT * FROM runtime_operations ORDER BY started_at_unix_ms DESC")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(OperationRecord {
                    id: row.try_get("id")?,
                    device_ref: serde_json::from_str(row.try_get("device_ref_json")?)?,
                    initiated_by: row.try_get("initiated_by")?,
                    kind: row.try_get("kind")?,
                    direction: row.try_get("direction")?,
                    source: row.try_get("source")?,
                    destination: row.try_get("destination")?,
                    overwrite: row.try_get("overwrite")?,
                    state: row.try_get("state")?,
                    offset: u64::try_from(row.try_get::<i64, _>("offset")?)
                        .map_err(|_| RuntimeStoreError::InvalidStoredOffset)?,
                    size: u64::try_from(row.try_get::<i64, _>("size")?)
                        .map_err(|_| RuntimeStoreError::InvalidStoredOffset)?,
                    started_at_unix_ms: row.try_get("started_at_unix_ms")?,
                    finished_at_unix_ms: row.try_get("finished_at_unix_ms")?,
                    message: row.try_get("message")?,
                })
            })
            .collect()
    }
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
