use std::time::{SystemTime, UNIX_EPOCH};

use pab_protocol::{OperatorRef, RequestId};

use super::{TaskStore, TaskStoreError};

impl TaskStore {
    pub async fn start_desktop_input(
        &self,
        request_id: RequestId,
        initiated_by: OperatorRef,
        kind: &str,
    ) -> Result<(), TaskStoreError> {
        sqlx::query(
            "INSERT INTO read_operations (request_id, initiated_by_json, kind, path, state, started_at_unix_ms) VALUES (?, ?, 'desktop_input', ?, 'running', ?)",
        )
        .bind(request_id.to_string())
        .bind(serde_json::to_string(&initiated_by)?)
        .bind(kind)
        .bind(now_unix_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn start_screenshot_read(
        &self,
        request_id: RequestId,
        initiated_by: OperatorRef,
    ) -> Result<(), TaskStoreError> {
        sqlx::query(
            "INSERT INTO read_operations (request_id, initiated_by_json, kind, path, state, started_at_unix_ms) VALUES (?, ?, 'screenshot', 'interactive_desktop', 'running', ?)",
        )
        .bind(request_id.to_string())
        .bind(serde_json::to_string(&initiated_by)?)
        .bind(now_unix_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn start_window_read(
        &self,
        request_id: RequestId,
        initiated_by: OperatorRef,
    ) -> Result<(), TaskStoreError> {
        sqlx::query(
            "INSERT INTO read_operations (request_id, initiated_by_json, kind, path, state, started_at_unix_ms) VALUES (?, ?, 'windows', '', 'running', ?)",
        )
        .bind(request_id.to_string())
        .bind(serde_json::to_string(&initiated_by)?)
        .bind(now_unix_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn interrupt_read_operations(&self) -> Result<(), TaskStoreError> {
        sqlx::query(
            "UPDATE read_operations SET state = CASE WHEN kind IN ('file_write', 'file_patch', 'mkdir', 'file_copy', 'file_move', 'file_delete', 'archive_create', 'archive_extract', 'process_terminate', 'service_control', 'window_focus', 'window_control', 'type_text', 'container_control', 'git_commit', 'git_checkout', 'git_fetch', 'git_pull', 'git_push') THEN 'unconfirmed' ELSE 'interrupted' END, finished_at_unix_ms = ?, message = 'Executor stopped before the operation result was confirmed; accepted mutations may have taken effect and are never replayed' WHERE state IN ('running', 'cancel_requested')",
        )
        .bind(now_unix_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn start_directory_read(
        &self,
        request_id: RequestId,
        initiated_by: OperatorRef,
        path: &str,
    ) -> Result<(), TaskStoreError> {
        sqlx::query(
            "INSERT INTO read_operations (request_id, initiated_by_json, kind, path, state, started_at_unix_ms) VALUES (?, ?, 'directory', ?, 'running', ?)",
        )
        .bind(request_id.to_string())
        .bind(serde_json::to_string(&initiated_by)?)
        .bind(path)
        .bind(now_unix_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_directory_read(
        &self,
        request_id: RequestId,
        state: &str,
        result_count: usize,
        message: Option<&str>,
    ) -> Result<(), TaskStoreError> {
        sqlx::query(
            "UPDATE read_operations SET state = ?, result_count = ?, finished_at_unix_ms = ?, message = ? WHERE request_id = ? AND state = 'running'",
        )
        .bind(state)
        .bind(i64::try_from(result_count).map_err(|_| TaskStoreError::OffsetTooLarge)?)
        .bind(now_unix_ms())
        .bind(message)
        .bind(request_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_window_read(
        &self,
        request_id: RequestId,
        state: &str,
        result_count: usize,
        message: Option<&str>,
    ) -> Result<(), TaskStoreError> {
        self.finish_directory_read(request_id, state, result_count, message)
            .await
    }
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use pab_protocol::UserId;
    use sqlx::Row;

    use super::*;

    #[tokio::test]
    async fn directory_attempt_and_result_survive_database_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("executor.sqlite3");
        let store = TaskStore::open(&path).await.unwrap();
        let id = RequestId::from_u128(79);
        let actor = OperatorRef::account(
            UserId::from_u128(7),
            pab_protocol::EndpointKey::new([7; 32]),
        );
        store
            .start_directory_read(id, actor, "/tmp/example")
            .await
            .unwrap();
        store
            .finish_directory_read(id, "completed", 3, None)
            .await
            .unwrap();
        drop(store);

        let reopened = TaskStore::open(&path).await.unwrap();
        let row = sqlx::query(
            "SELECT initiated_by_json, path, state, result_count, finished_at_unix_ms FROM read_operations WHERE request_id = ?",
        )
        .bind(id.to_string())
        .fetch_one(&reopened.pool)
        .await
        .unwrap();
        assert_eq!(row.try_get::<String, _>("path").unwrap(), "/tmp/example");
        assert_eq!(row.try_get::<String, _>("state").unwrap(), "completed");
        assert_eq!(row.try_get::<i64, _>("result_count").unwrap(), 3);
        assert!(
            row.try_get::<Option<i64>, _>("finished_at_unix_ms")
                .unwrap()
                .is_some()
        );
        let stored_actor: OperatorRef =
            serde_json::from_str(row.try_get("initiated_by_json").unwrap()).unwrap();
        assert_eq!(stored_actor, actor);

        let interrupted_id = RequestId::from_u128(80);
        reopened
            .start_directory_read(interrupted_id, actor, "/tmp/interrupted")
            .await
            .unwrap();
        reopened.interrupt_read_operations().await.unwrap();
        let state: String =
            sqlx::query_scalar("SELECT state FROM read_operations WHERE request_id = ?")
                .bind(interrupted_id.to_string())
                .fetch_one(&reopened.pool)
                .await
                .unwrap();
        assert_eq!(state, "interrupted");
    }
}
