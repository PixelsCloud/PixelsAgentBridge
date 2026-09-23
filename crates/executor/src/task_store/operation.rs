use std::time::{SystemTime, UNIX_EPOCH};

use pab_protocol::{OperatorRef, RequestId};

use super::{TaskStore, TaskStoreError};

impl TaskStore {
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
    ) -> Result<(), TaskStoreError> {
        sqlx::query(
            "INSERT INTO transfer_operations (request_id, initiated_by_json, direction, path, size, state, started_at_unix_ms) VALUES (?, ?, ?, ?, ?, 'running', ?)",
        )
        .bind(request_id.to_string())
        .bind(serde_json::to_string(&initiated_by)?)
        .bind(direction)
        .bind(path)
        .bind(i64::try_from(size).map_err(|_| TaskStoreError::OffsetTooLarge)?)
        .bind(now_unix_ms())
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
