use pab_protocol::{DeviceCode, DeviceRef, RequestId};
use sqlx::Row;

use super::operation::now_unix_ms;
use super::store::{RuntimeStore, RuntimeStoreError};

pub struct TerminalAuditEvent {
    pub sequence: u64,
    pub kind: String,
    pub payload: Vec<u8>,
    pub state: String,
    pub at_unix_ms: i64,
}

impl RuntimeStore {
    pub async fn start_terminal_operation(
        &self,
        id: RequestId,
        device_ref: DeviceRef,
        device_code: Option<DeviceCode>,
        initiated_by: &str,
        destination: &str,
        owner_session_id: &str,
    ) -> Result<(), RuntimeStoreError> {
        sqlx::query(
            r#"
            INSERT INTO runtime_operations (
                id, device_ref_json, device_code, initiated_by, kind, direction,
                source, destination, overwrite, state, started_at_unix_ms, owner_session_id
            ) VALUES (?, ?, ?, ?, 'terminal', 'bidirectional', 'interactive_shell', ?, 0, 'running', ?, ?)
            "#,
        )
        .bind(id.to_string())
        .bind(serde_json::to_string(&device_ref)?)
        .bind(device_code.map(|code| code.to_string()))
        .bind(initiated_by)
        .bind(destination)
        .bind(now_unix_ms())
        .bind(owner_session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn begin_terminal_event(
        &self,
        id: RequestId,
        sequence: u64,
        kind: &str,
        payload: &[u8],
    ) -> Result<(), RuntimeStoreError> {
        let result = sqlx::query(
            r#"
            INSERT INTO runtime_terminal_events (session_id, seq, kind, payload, state, at_unix_ms)
            SELECT id, ?, ?, ?, 'pending', ?
            FROM runtime_operations
            WHERE id = ? AND kind = 'terminal' AND state = 'running'
            "#,
        )
        .bind(i64::try_from(sequence).map_err(|_| RuntimeStoreError::OffsetTooLarge)?)
        .bind(kind)
        .bind(payload)
        .bind(now_unix_ms())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(RuntimeStoreError::NotFound);
        }
        Ok(())
    }

    pub async fn finish_terminal_event(
        &self,
        id: RequestId,
        sequence: u64,
        state: &str,
    ) -> Result<(), RuntimeStoreError> {
        sqlx::query(
            r#"
            UPDATE runtime_terminal_events
            SET state = ?
            WHERE session_id = ? AND seq = ? AND state = 'pending'
            "#,
        )
        .bind(state)
        .bind(id.to_string())
        .bind(i64::try_from(sequence).map_err(|_| RuntimeStoreError::OffsetTooLarge)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn terminal_events(
        &self,
        id: &str,
    ) -> Result<Vec<TerminalAuditEvent>, RuntimeStoreError> {
        let rows = sqlx::query(
            r#"
            SELECT seq, kind, payload, state, at_unix_ms
            FROM runtime_terminal_events
            WHERE session_id = ?
            ORDER BY seq
            "#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(TerminalAuditEvent {
                    sequence: u64::try_from(row.try_get::<i64, _>("seq")?)
                        .map_err(|_| RuntimeStoreError::InvalidStoredOffset)?,
                    kind: row.try_get("kind")?,
                    payload: row.try_get("payload")?,
                    state: row.try_get("state")?,
                    at_unix_ms: row.try_get("at_unix_ms")?,
                })
            })
            .collect()
    }
}
