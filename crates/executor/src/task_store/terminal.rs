use std::time::{SystemTime, UNIX_EPOCH};

use pab_protocol::{OperatorRef, RequestId};

use super::{TaskStore, TaskStoreError};

impl TaskStore {
    pub async fn interrupt_terminals(&self) -> Result<(), TaskStoreError> {
        sqlx::query(
            r#"
            UPDATE terminal_sessions
            SET state = 'interrupted', finished_at_unix_ms = ?,
                message = 'Executor restarted during terminal session'
            WHERE state = 'running'
            "#,
        )
        .bind(now_unix_ms())
        .execute(&self.pool)
        .await?;
        sqlx::query("UPDATE terminal_events SET state = 'interrupted' WHERE state = 'pending'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn start_terminal(
        &self,
        id: RequestId,
        initiated_by: OperatorRef,
        shell: &str,
        connection: RequestId,
        selection: pab_protocol::ExecutionSelection,
        identity: Option<&pab_protocol::ExecutionIdentity>,
        cols: u16,
        rows: u16,
    ) -> Result<(), TaskStoreError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            INSERT INTO terminal_sessions (id, initiated_by_json, shell, state, started_at_unix_ms)
            VALUES (?, ?, ?, 'running', ?)
            "#,
        )
        .bind(id.to_string())
        .bind(serde_json::to_string(&initiated_by)?)
        .bind(shell)
        .bind(now_unix_ms())
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO terminal_execution (session_id, connection_id, selection_json, identity_json, cols, rows) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(id.to_string()).bind(connection.to_string()).bind(serde_json::to_string(&selection)?).bind(identity.map(serde_json::to_string).transpose()?).bind(i64::from(cols)).bind(i64::from(rows)).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn begin_terminal_event(
        &self,
        id: RequestId,
        sequence: u64,
        kind: &str,
        payload: &[u8],
    ) -> Result<(), TaskStoreError> {
        let result = sqlx::query(
            r#"
            INSERT INTO terminal_events (session_id, seq, kind, payload, state, at_unix_ms)
            SELECT id, ?, ?, ?, 'pending', ?
            FROM terminal_sessions
            WHERE id = ? AND state = 'running'
            "#,
        )
        .bind(i64::try_from(sequence).map_err(|_| TaskStoreError::OffsetTooLarge)?)
        .bind(kind)
        .bind(payload)
        .bind(now_unix_ms())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(TaskStoreError::NotFound);
        }
        Ok(())
    }

    pub async fn finish_terminal_event(
        &self,
        id: RequestId,
        sequence: u64,
        state: &str,
    ) -> Result<(), TaskStoreError> {
        sqlx::query(
            r#"
            UPDATE terminal_events
            SET state = ?
            WHERE session_id = ? AND seq = ? AND state = 'pending'
            "#,
        )
        .bind(state)
        .bind(id.to_string())
        .bind(i64::try_from(sequence).map_err(|_| TaskStoreError::OffsetTooLarge)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_terminal(
        &self,
        id: RequestId,
        state: &str,
        message: Option<&str>,
    ) -> Result<(), TaskStoreError> {
        sqlx::query(
            r#"
            UPDATE terminal_sessions
            SET state = ?, finished_at_unix_ms = ?, message = ?
            WHERE id = ? AND state = 'running'
            "#,
        )
        .bind(state)
        .bind(now_unix_ms())
        .bind(message)
        .bind(id.to_string())
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
