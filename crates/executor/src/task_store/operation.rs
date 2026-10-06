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
            "SELECT o.initiated_by_json, o.direction, o.path, o.state, o.offset, o.size, o.sha256, o.finished_at_unix_ms, o.message, e.context_json FROM transfer_operations o LEFT JOIN transfer_execution e ON e.request_id=o.request_id WHERE o.request_id = ?",
        )
        .bind(request_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(TaskStoreError::NotFound)?;
        let stored_actor: OperatorRef = serde_json::from_str(row.try_get("initiated_by_json")?)?;
        if stored_actor != initiated_by {
            return Err(TaskStoreError::NotFound);
        }
        let mut snapshot = TransferSnapshot {
            execution_context: row
                .try_get::<Option<String>, _>("context_json")?
                .map(|value| serde_json::from_str(&value))
                .transpose()?,
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
            published: match (
                row.try_get::<String, _>("direction")?.as_str(),
                row.try_get::<String, _>("state")?.as_str(),
            ) {
                ("receive", "completed") => Some(true),
                ("receive", "failed" | "interrupted" | "cancelled") => Some(false),
                _ => None,
            },
        };
        // A process can stop between publishing a verified file and recording
        // completion. Resolve that window using file evidence, never by replay.
        if snapshot.direction == "receive"
            && snapshot.state == "committing"
            && snapshot.execution_context.as_ref().is_none_or(|context| {
                context
                    .identity
                    .as_ref()
                    .is_none_or(|identity| identity.mode == pab_protocol::ExecutionMode::Service)
            })
            && publication_matches(&snapshot).await
        {
            self.finish_transfer(request_id, "completed", None).await?;
            snapshot.state = "completed".to_owned();
            snapshot.finished_at_unix_ms = Some(now_unix_ms());
            snapshot.published = Some(true);
        }
        Ok(snapshot)
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

    pub async fn begin_transfer_publication(&self, id: RequestId) -> Result<(), TaskStoreError> {
        let result = sqlx::query("UPDATE transfer_operations SET state = 'committing' WHERE request_id = ? AND state = 'running' AND offset = size")
            .bind(id.to_string()).execute(&self.pool).await?;
        if result.rows_affected() != 1 {
            return Err(TaskStoreError::NotFound);
        }
        Ok(())
    }

    pub async fn publication_failed(
        &self,
        id: RequestId,
        message: &str,
    ) -> Result<(), TaskStoreError> {
        sqlx::query("UPDATE transfer_operations SET state = 'failed', message = ?, finished_at_unix_ms = ? WHERE request_id = ? AND state = 'committing'")
            .bind(message).bind(now_unix_ms()).bind(id.to_string()).execute(&self.pool).await?;
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
            "UPDATE transfer_operations SET state = ?, message = ?, finished_at_unix_ms = ? WHERE request_id = ? AND (state = 'running' OR (state = 'committing' AND ? = 'completed'))",
        )
        .bind(state)
        .bind(message)
        .bind(now_unix_ms())
        .bind(request_id.to_string())
        .bind(state)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

async fn publication_matches(snapshot: &TransferSnapshot) -> bool {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncReadExt;
    let Some(expected) = snapshot.sha256.as_deref() else {
        return false;
    };
    let Ok(metadata) = tokio::fs::symlink_metadata(&snapshot.path).await else {
        return false;
    };
    if !metadata.is_file() || metadata.len() != snapshot.size {
        return false;
    }
    let Ok(mut file) = tokio::fs::File::open(&snapshot.path).await else {
        return false;
    };
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    let mut size = 0_u64;
    loop {
        let Ok(read) = file.read(&mut buffer).await else {
            return false;
        };
        if read == 0 {
            break;
        }
        size += read as u64;
        if size > snapshot.size {
            return false;
        }
        digest.update(&buffer[..read]);
    }
    size == snapshot.size && format!("{:x}", digest.finalize()) == expected
}

pub(super) fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
