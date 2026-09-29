use std::time::{SystemTime, UNIX_EPOCH};

use pab_protocol::{OperatorRef, RequestId, TransferSnapshot};
use sqlx::Row;

use super::{TaskStore, TaskStoreError};

impl TaskStore {
    pub async fn get_transfer(
        &self,
        initiated_by: OperatorRef,
        request_id: RequestId,
    ) -> Result<TransferSnapshot, TaskStoreError> {
        let row = sqlx::query(
            "SELECT initiated_by_json, direction, path, state, offset, size, sha256, finished_at_unix_ms, message FROM transfer_operations WHERE request_id = ?",
        )
        .bind(request_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(TaskStoreError::NotFound)?;
        let stored_actor: OperatorRef = serde_json::from_str(row.try_get("initiated_by_json")?)?;
        if stored_actor != initiated_by {
            return Err(TaskStoreError::NotFound);
        }
        Ok(TransferSnapshot {
            request_id,
            initiated_by: stored_actor,
            direction: row.try_get("direction")?,
            path: row.try_get("path")?,
            state: row.try_get("state")?,
            offset: u64::try_from(row.try_get::<i64, _>("offset")?)
                .map_err(|_| TaskStoreError::OffsetTooLarge)?,
            size: u64::try_from(row.try_get::<i64, _>("size")?)
                .map_err(|_| TaskStoreError::OffsetTooLarge)?,
            sha256: row.try_get("sha256")?,
            finished_at_unix_ms: row.try_get("finished_at_unix_ms")?,
            message: row.try_get("message")?,
        })
    }

    pub async fn interrupt_transfers(&self) -> Result<(), TaskStoreError> {
        sqlx::query(
            "UPDATE transfer_operations SET state = 'interrupted', finished_at_unix_ms = ?, message = 'Executor stopped during transfer' WHERE state = 'running'",
        )
        .bind(now_unix_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn start_transfer(
        &self,
        request_id: RequestId,
        initiated_by: OperatorRef,
        direction: &str,
        path: &str,
        size: u64,
        sha256: Option<&str>,
    ) -> Result<(), TaskStoreError> {
        sqlx::query(
            "INSERT INTO transfer_operations (request_id, initiated_by_json, direction, path, size, sha256, state, started_at_unix_ms) VALUES (?, ?, ?, ?, ?, ?, 'running', ?)",
        )
        .bind(request_id.to_string())
        .bind(serde_json::to_string(&initiated_by)?)
        .bind(direction)
        .bind(path)
        .bind(i64::try_from(size).map_err(|_| TaskStoreError::OffsetTooLarge)?)
        .bind(sha256)
        .bind(now_unix_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn transfer_hash(
        &self,
        request_id: RequestId,
        sha256: &str,
    ) -> Result<(), TaskStoreError> {
        sqlx::query(
            "UPDATE transfer_operations SET sha256 = ? WHERE request_id = ? AND state = 'running'",
        )
        .bind(sha256)
        .bind(request_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn transfer_progress(
        &self,
        request_id: RequestId,
        offset: u64,
        size: u64,
    ) -> Result<(), TaskStoreError> {
        sqlx::query(
            "UPDATE transfer_operations SET offset = MAX(offset, ?), size = MAX(size, ?) WHERE request_id = ? AND state = 'running'",
        )
        .bind(i64::try_from(offset).map_err(|_| TaskStoreError::OffsetTooLarge)?)
        .bind(i64::try_from(size).map_err(|_| TaskStoreError::OffsetTooLarge)?)
        .bind(request_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_transfer(
        &self,
        request_id: RequestId,
        state: &str,
        message: Option<&str>,
    ) -> Result<(), TaskStoreError> {
        sqlx::query(
            "UPDATE transfer_operations SET state = ?, message = ?, finished_at_unix_ms = ? WHERE request_id = ? AND state = 'running'",
        )
        .bind(state)
        .bind(message)
        .bind(now_unix_ms())
        .bind(request_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
