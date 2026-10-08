use std::time::{SystemTime, UNIX_EPOCH};

use pab_protocol::{DeviceCode, DeviceRef, RequestId};
use sqlx::{Row, sqlite::SqliteRow};

use super::store::{RuntimeStore, RuntimeStoreError};

const SESSION_STALE_AFTER_MS: i64 = 15_000;

#[derive(Debug, Clone, serde::Serialize)]
pub struct OperationRecord {
    pub initiating_user: Option<pab_protocol::UserAttribution>,
    #[serde(skip)]
    pub execution_identity: Option<pab_protocol::ExecutionIdentity>,
    #[serde(skip)]
    pub ui: Option<pab_protocol::UiOperationSummary>,
    pub id: String,
    pub device_ref: DeviceRef,
    pub device_code: Option<DeviceCode>,
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
    pub execution_observation: Option<String>,
    #[serde(skip)]
    pub transfer_phase: Option<String>,
    #[serde(skip)]
    pub filesystem_mutation: Option<pab_protocol::FileMutationSummary>,
}

impl RuntimeStore {
    pub async fn start_desktop_input_operation(
        &self,
        id: RequestId,
        device_ref: DeviceRef,
        device_code: Option<DeviceCode>,
        initiated_by: &str,
        kind: &str,
        owner_session_id: &str,
    ) -> Result<(), RuntimeStoreError> {
        sqlx::query(
            "INSERT INTO runtime_operations (id, device_ref_json, device_code, initiated_by, kind, direction, source, destination, overwrite, state, started_at_unix_ms, owner_session_id) VALUES (?, ?, ?, ?, 'desktop_input', 'send', ?, '', 0, 'running', ?, ?)",
        )
        .bind(id.to_string())
        .bind(serde_json::to_string(&device_ref)?)
        .bind(device_code.map(|code| code.to_string()))
        .bind(initiated_by)
        .bind(kind)
        .bind(now_unix_ms())
        .bind(owner_session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn start_screenshot_operation(
        &self,
        id: RequestId,
        device_ref: DeviceRef,
        device_code: Option<DeviceCode>,
        initiated_by: &str,
        destination: &str,
        owner_session_id: &str,
    ) -> Result<(), RuntimeStoreError> {
        sqlx::query(
            "INSERT INTO runtime_operations (id, device_ref_json, device_code, initiated_by, kind, direction, source, destination, overwrite, state, started_at_unix_ms, owner_session_id) VALUES (?, ?, ?, ?, 'screenshot', 'download', 'interactive_desktop', ?, 0, 'running', ?, ?)",
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

    pub async fn start_window_operation(
        &self,
        id: RequestId,
        device_ref: DeviceRef,
        device_code: Option<DeviceCode>,
        initiated_by: &str,
        owner_session_id: &str,
    ) -> Result<(), RuntimeStoreError> {
        sqlx::query(
            "INSERT INTO runtime_operations (id, device_ref_json, device_code, initiated_by, kind, direction, source, destination, overwrite, state, started_at_unix_ms, owner_session_id) VALUES (?, ?, ?, ?, 'windows', 'read', '', '', 0, 'running', ?, ?)",
        )
        .bind(id.to_string())
        .bind(serde_json::to_string(&device_ref)?)
        .bind(device_code.map(|code| code.to_string()))
        .bind(initiated_by)
        .bind(now_unix_ms())
        .bind(owner_session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn start_directory_operation(
        &self,
        id: RequestId,
        device_ref: DeviceRef,
        device_code: Option<DeviceCode>,
        initiated_by: &str,
        path: &str,
        owner_session_id: &str,
    ) -> Result<(), RuntimeStoreError> {
        sqlx::query(
            "INSERT INTO runtime_operations (id, device_ref_json, device_code, initiated_by, kind, direction, source, destination, overwrite, state, started_at_unix_ms, owner_session_id) VALUES (?, ?, ?, ?, 'directory', 'read', ?, '', 0, 'running', ?, ?)",
        )
        .bind(id.to_string())
        .bind(serde_json::to_string(&device_ref)?)
        .bind(device_code.map(|code| code.to_string()))
        .bind(initiated_by)
        .bind(path)
        .bind(now_unix_ms())
        .bind(owner_session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn start_operation(
        &self,
        id: RequestId,
        device_ref: DeviceRef,
        device_code: Option<DeviceCode>,
        initiated_by: &str,
        direction: &str,
        source: &str,
        destination: &str,
        overwrite: bool,
        owner_session_id: Option<&str>,
    ) -> Result<(), RuntimeStoreError> {
        let device_ref_json = serde_json::to_string(&device_ref)?;
        let device_code = device_code.map(|code| code.to_string());
        let result = sqlx::query(
            "INSERT INTO runtime_operations (id, device_ref_json, device_code, initiated_by, kind, direction, source, destination, overwrite, state, started_at_unix_ms, owner_session_id) VALUES (?, ?, ?, ?, 'file_transfer', ?, ?, ?, ?, 'running', ?, ?) ON CONFLICT(id) DO NOTHING",
        )
        .bind(id.to_string())
        .bind(&device_ref_json)
        .bind(&device_code)
        .bind(initiated_by)
        .bind(direction)
        .bind(source)
        .bind(destination)
        .bind(overwrite)
        .bind(now_unix_ms())
        .bind(owner_session_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            let row = sqlx::query(
                "SELECT device_ref_json, device_code, initiated_by, kind, direction, source, destination, overwrite, state, owner_session_id FROM runtime_operations WHERE id = ?",
            )
            .bind(id.to_string())
            .fetch_one(&self.pool)
            .await?;
            let same_operation = row.try_get::<String, _>("device_ref_json")? == device_ref_json
                && row.try_get::<Option<String>, _>("device_code")? == device_code
                && row.try_get::<String, _>("initiated_by")? == initiated_by
                && row.try_get::<String, _>("kind")? == "file_transfer"
                && row.try_get::<String, _>("direction")? == direction
                && row.try_get::<String, _>("source")? == source
                && row.try_get::<String, _>("destination")? == destination
                && row.try_get::<bool, _>("overwrite")? == overwrite
                && row
                    .try_get::<Option<String>, _>("owner_session_id")?
                    .as_deref()
                    == owner_session_id
                && matches!(
                    row.try_get::<String, _>("state")?.as_str(),
                    "running" | "cancel_requested"
                );
            if !same_operation {
                return Err(RuntimeStoreError::RequestConflict);
            }
        }
        Ok(())
    }

    pub async fn operation_progress(
        &self,
        id: RequestId,
        offset: u64,
        size: u64,
    ) -> Result<(), RuntimeStoreError> {
        if offset > size {
            return Err(RuntimeStoreError::InvalidStoredOffset);
        }
        sqlx::query(
            "UPDATE runtime_operations SET offset = MAX(offset, ?), size = MAX(size, ?) WHERE id = ? AND state IN ('running', 'cancel_requested')",
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
    ) -> Result<bool, RuntimeStoreError> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE runtime_operations SET state = ?, message = ?, finished_at_unix_ms = ? WHERE id = ? AND state IN ('running', 'cancel_requested')",
        )
        .bind(state)
        .bind(message)
        .bind(now_unix_ms())
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 1 {
            sqlx::query("DELETE FROM runtime_download_claims WHERE operation_id = ?")
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "UPDATE runtime_async_transfers SET phase = ?, updated_at_unix_ms = ? WHERE id = ?",
            )
            .bind(state)
            .bind(now_unix_ms())
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn request_transfer_cancellation(
        &self,
        id: RequestId,
        owner_session_id: &str,
    ) -> Result<bool, RuntimeStoreError> {
        let result = sqlx::query(
            "UPDATE runtime_operations SET state = 'cancel_requested' WHERE id = ? AND owner_session_id = ? AND state = 'running'",
        )
        .bind(id.to_string())
        .bind(owner_session_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn operations(&self) -> Result<Vec<OperationRecord>, RuntimeStoreError> {
        let rows = sqlx::query(
            "SELECT o.*, (SELECT u.user_json FROM runtime_operation_users u WHERE u.operation_id=o.id) AS initiating_user, (SELECT i.identity_json FROM runtime_operation_identity i WHERE i.id=o.id) AS execution_identity, COALESCE((SELECT a.phase FROM runtime_async_transfers a WHERE a.id = o.id), (SELECT json_extract(f.reply_json, '$.state') FROM runtime_filesystem_results f WHERE f.id = o.id), (SELECT json_extract(q.reply_json, '$.state') FROM runtime_system_results q WHERE q.id = o.id)) AS phase, (SELECT json_extract(f.reply_json, '$.mutation') FROM runtime_filesystem_results f WHERE f.id = o.id) AS filesystem_mutation, (SELECT json_extract(q.reply_json, '$.data.snapshot.ui') FROM runtime_system_results q WHERE q.id = o.id) AS ui_snapshot, s.heartbeat_at_unix_ms, s.stopped_at_unix_ms FROM runtime_operations o LEFT JOIN runtime_sessions s ON s.id = o.owner_session_id ORDER BY o.started_at_unix_ms DESC, o.id DESC",
        )
            .fetch_all(&self.pool)
            .await?;
        let stale_before = now_unix_ms().saturating_sub(SESSION_STALE_AFTER_MS);
        rows.into_iter()
            .map(|row| decode_operation(row, stale_before))
            .collect()
    }

    pub(super) async fn operations_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<OperationRecord>, RuntimeStoreError> {
        let rows = sqlx::query(
            "SELECT o.*, (SELECT u.user_json FROM runtime_operation_users u WHERE u.operation_id=o.id) AS initiating_user, (SELECT i.identity_json FROM runtime_operation_identity i WHERE i.id=o.id) AS execution_identity, COALESCE((SELECT a.phase FROM runtime_async_transfers a WHERE a.id = o.id), (SELECT json_extract(f.reply_json, '$.state') FROM runtime_filesystem_results f WHERE f.id = o.id), (SELECT json_extract(q.reply_json, '$.state') FROM runtime_system_results q WHERE q.id = o.id)) AS phase, (SELECT json_extract(f.reply_json, '$.mutation') FROM runtime_filesystem_results f WHERE f.id = o.id) AS filesystem_mutation, (SELECT json_extract(q.reply_json, '$.data.snapshot.ui') FROM runtime_system_results q WHERE q.id = o.id) AS ui_snapshot, s.heartbeat_at_unix_ms, s.stopped_at_unix_ms FROM runtime_operations o \
             LEFT JOIN runtime_sessions s ON s.id = o.owner_session_id \
             WHERE o.owner_session_id = ? AND (o.finished_at_unix_ms IS NULL OR o.id IN \
               (SELECT id FROM runtime_operations WHERE owner_session_id = ? AND finished_at_unix_ms IS NOT NULL \
                ORDER BY started_at_unix_ms DESC, id DESC LIMIT 100)) \
             ORDER BY o.started_at_unix_ms DESC, o.id DESC",
        ).bind(session_id).bind(session_id).fetch_all(&self.pool).await?;
        let stale_before = now_unix_ms().saturating_sub(SESSION_STALE_AFTER_MS);
        rows.into_iter()
            .map(|row| decode_operation(row, stale_before))
            .collect()
    }

    pub async fn operations_page(
        &self,
        before: Option<(i64, &str)>,
        limit: u32,
    ) -> Result<Vec<OperationRecord>, RuntimeStoreError> {
        let stale_before = now_unix_ms().saturating_sub(SESSION_STALE_AFTER_MS);
        let rows = sqlx::query(
            "SELECT o.*, (SELECT u.user_json FROM runtime_operation_users u WHERE u.operation_id=o.id) AS initiating_user, (SELECT i.identity_json FROM runtime_operation_identity i WHERE i.id=o.id) AS execution_identity, COALESCE((SELECT a.phase FROM runtime_async_transfers a WHERE a.id = o.id), (SELECT json_extract(f.reply_json, '$.state') FROM runtime_filesystem_results f WHERE f.id = o.id), (SELECT json_extract(q.reply_json, '$.state') FROM runtime_system_results q WHERE q.id = o.id)) AS phase, (SELECT json_extract(f.reply_json, '$.mutation') FROM runtime_filesystem_results f WHERE f.id = o.id) AS filesystem_mutation, (SELECT json_extract(q.reply_json, '$.data.snapshot.ui') FROM runtime_system_results q WHERE q.id = o.id) AS ui_snapshot, s.heartbeat_at_unix_ms, s.stopped_at_unix_ms FROM runtime_operations o LEFT JOIN runtime_sessions s ON s.id = o.owner_session_id WHERE (? IS NULL OR (o.started_at_unix_ms, o.id) < (?, ?)) ORDER BY o.started_at_unix_ms DESC, o.id DESC LIMIT ?",
        )
        .bind(before.map(|value| value.0))
        .bind(before.map(|value| value.0))
        .bind(before.map(|value| value.1))
        .bind(i64::from(limit.min(101)))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| decode_operation(row, stale_before))
            .collect()
    }

    pub async fn operations_page_for_device(
        &self,
        device_ref: pab_protocol::DeviceRef,
        before: Option<(i64, &str)>,
        limit: u32,
    ) -> Result<Vec<OperationRecord>, RuntimeStoreError> {
        let stale_before = now_unix_ms().saturating_sub(SESSION_STALE_AFTER_MS);
        let rows = sqlx::query(
            "SELECT o.*, (SELECT u.user_json FROM runtime_operation_users u WHERE u.operation_id=o.id) AS initiating_user, (SELECT i.identity_json FROM runtime_operation_identity i WHERE i.id=o.id) AS execution_identity, COALESCE((SELECT a.phase FROM runtime_async_transfers a WHERE a.id = o.id), (SELECT json_extract(f.reply_json, '$.state') FROM runtime_filesystem_results f WHERE f.id = o.id), (SELECT json_extract(q.reply_json, '$.state') FROM runtime_system_results q WHERE q.id = o.id)) AS phase, (SELECT json_extract(f.reply_json, '$.mutation') FROM runtime_filesystem_results f WHERE f.id = o.id) AS filesystem_mutation, (SELECT json_extract(q.reply_json, '$.data.snapshot.ui') FROM runtime_system_results q WHERE q.id = o.id) AS ui_snapshot, s.heartbeat_at_unix_ms, s.stopped_at_unix_ms FROM runtime_operations o LEFT JOIN runtime_sessions s ON s.id = o.owner_session_id WHERE o.device_ref_json = ? AND (? IS NULL OR (o.started_at_unix_ms, o.id) < (?, ?)) ORDER BY o.started_at_unix_ms DESC, o.id DESC LIMIT ?",
        )
        .bind(serde_json::to_string(&device_ref)?)
        .bind(before.map(|value| value.0))
        .bind(before.map(|value| value.0))
        .bind(before.map(|value| value.1))
        .bind(i64::from(limit.min(101)))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| decode_operation(row, stale_before))
            .collect()
    }

    pub async fn operations_refresh(&self) -> Result<Vec<OperationRecord>, RuntimeStoreError> {
        let now = now_unix_ms();
        let stale_before = now.saturating_sub(SESSION_STALE_AFTER_MS);
        let recent_finish = now.saturating_sub(5 * 60 * 1_000);
        let rows = sqlx::query(
            "SELECT o.*, (SELECT u.user_json FROM runtime_operation_users u WHERE u.operation_id=o.id) AS initiating_user, (SELECT i.identity_json FROM runtime_operation_identity i WHERE i.id=o.id) AS execution_identity, COALESCE((SELECT a.phase FROM runtime_async_transfers a WHERE a.id = o.id), (SELECT json_extract(f.reply_json, '$.state') FROM runtime_filesystem_results f WHERE f.id = o.id), (SELECT json_extract(q.reply_json, '$.state') FROM runtime_system_results q WHERE q.id = o.id)) AS phase, (SELECT json_extract(f.reply_json, '$.mutation') FROM runtime_filesystem_results f WHERE f.id = o.id) AS filesystem_mutation, (SELECT json_extract(q.reply_json, '$.data.snapshot.ui') FROM runtime_system_results q WHERE q.id = o.id) AS ui_snapshot, s.heartbeat_at_unix_ms, s.stopped_at_unix_ms FROM runtime_operations o LEFT JOIN runtime_sessions s ON s.id = o.owner_session_id WHERE o.state IN ('running', 'cancel_requested') OR o.finished_at_unix_ms >= ? OR o.id IN (SELECT id FROM runtime_operations ORDER BY started_at_unix_ms DESC, id DESC LIMIT 40) ORDER BY (o.state IN ('running', 'cancel_requested')) DESC, o.finished_at_unix_ms DESC, o.started_at_unix_ms DESC, o.id DESC LIMIT 200",
        )
        .bind(recent_finish)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| decode_operation(row, stale_before))
            .collect()
    }

    pub async fn unconfirmed_transfers(
        &self,
        limit: u32,
        cursor: Option<(i64, &str)>,
    ) -> Result<Vec<OperationRecord>, RuntimeStoreError> {
        let stale_before = now_unix_ms().saturating_sub(SESSION_STALE_AFTER_MS);
        let rows = sqlx::query(
            "SELECT o.*, (SELECT u.user_json FROM runtime_operation_users u WHERE u.operation_id=o.id) AS initiating_user, (SELECT i.identity_json FROM runtime_operation_identity i WHERE i.id=o.id) AS execution_identity, COALESCE((SELECT a.phase FROM runtime_async_transfers a WHERE a.id = o.id), (SELECT json_extract(f.reply_json, '$.state') FROM runtime_filesystem_results f WHERE f.id = o.id), (SELECT json_extract(q.reply_json, '$.state') FROM runtime_system_results q WHERE q.id = o.id)) AS phase, (SELECT json_extract(f.reply_json, '$.mutation') FROM runtime_filesystem_results f WHERE f.id = o.id) AS filesystem_mutation, (SELECT json_extract(q.reply_json, '$.data.snapshot.ui') FROM runtime_system_results q WHERE q.id = o.id) AS ui_snapshot, s.heartbeat_at_unix_ms, s.stopped_at_unix_ms FROM runtime_operations o LEFT JOIN runtime_sessions s ON s.id = o.owner_session_id WHERE o.kind = 'file_transfer' AND o.state = 'running' AND o.owner_session_id IS NOT NULL AND (s.id IS NULL OR s.stopped_at_unix_ms IS NOT NULL OR s.heartbeat_at_unix_ms < ?) AND (? IS NULL OR (o.started_at_unix_ms, o.id) < (?, ?)) ORDER BY o.started_at_unix_ms DESC, o.id DESC LIMIT ?",
        )
        .bind(stale_before)
        .bind(cursor.map(|value| value.0))
        .bind(cursor.map(|value| value.0))
        .bind(cursor.map(|value| value.1))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| decode_operation(row, stale_before))
            .collect()
    }

    pub async fn cancellation_requests(
        &self,
        limit: u32,
        cursor: Option<(i64, &str)>,
    ) -> Result<Vec<OperationRecord>, RuntimeStoreError> {
        let stale_before = now_unix_ms().saturating_sub(SESSION_STALE_AFTER_MS);
        let rows = sqlx::query(
            "SELECT o.*, (SELECT u.user_json FROM runtime_operation_users u WHERE u.operation_id=o.id) AS initiating_user, (SELECT i.identity_json FROM runtime_operation_identity i WHERE i.id=o.id) AS execution_identity, COALESCE((SELECT a.phase FROM runtime_async_transfers a WHERE a.id = o.id), (SELECT json_extract(f.reply_json, '$.state') FROM runtime_filesystem_results f WHERE f.id = o.id), (SELECT json_extract(q.reply_json, '$.state') FROM runtime_system_results q WHERE q.id = o.id)) AS phase, (SELECT json_extract(f.reply_json, '$.mutation') FROM runtime_filesystem_results f WHERE f.id = o.id) AS filesystem_mutation, (SELECT json_extract(q.reply_json, '$.data.snapshot.ui') FROM runtime_system_results q WHERE q.id = o.id) AS ui_snapshot, s.heartbeat_at_unix_ms, s.stopped_at_unix_ms FROM runtime_operations o LEFT JOIN runtime_sessions s ON s.id = o.owner_session_id WHERE o.kind = 'file_transfer' AND o.state = 'cancel_requested' AND (? IS NULL OR (o.started_at_unix_ms, o.id) < (?, ?)) ORDER BY o.started_at_unix_ms DESC, o.id DESC LIMIT ?",
        )
        .bind(cursor.map(|value| value.0))
        .bind(cursor.map(|value| value.0))
        .bind(cursor.map(|value| value.1))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| decode_operation(row, stale_before))
            .collect()
    }
}

pub(super) fn decode_operation(
    row: SqliteRow,
    stale_before: i64,
) -> Result<OperationRecord, RuntimeStoreError> {
    let state: String = row.try_get("state")?;
    let owner_session_id: Option<String> = row.try_get("owner_session_id")?;
    let heartbeat_at: Option<i64> = row.try_get("heartbeat_at_unix_ms")?;
    let stopped_at: Option<i64> = row.try_get("stopped_at_unix_ms")?;
    let phase: Option<String> = row.try_get("phase").unwrap_or(None);
    let execution_observation = if state == "cancel_requested"
        || (state == "running" && phase.as_deref() == Some("unconfirmed"))
    {
        Some("unconfirmed".to_owned())
    } else if state != "running" {
        None
    } else if owner_session_id.is_none() {
        Some("unknown".to_owned())
    } else if stopped_at.is_some() || heartbeat_at.is_none_or(|value| value < stale_before) {
        Some("unconfirmed".to_owned())
    } else {
        Some("active".to_owned())
    };
    Ok(OperationRecord {
        initiating_user: row
            .try_get::<Option<String>, _>("initiating_user")?
            .map(|json| serde_json::from_str::<Option<pab_protocol::UserAttribution>>(&json))
            .transpose()?
            .flatten(),
        execution_identity: row
            .try_get::<Option<String>, _>("execution_identity")
            .unwrap_or(None)
            .map(|s| serde_json::from_str(&s))
            .transpose()?,
        ui: row
            .try_get::<Option<String>, _>("ui_snapshot")
            .unwrap_or(None)
            .map(|s| {
                serde_json::from_str::<pab_protocol::UiSnapshot>(&s)
                    .map(pab_protocol::UiOperationSummary::from)
            })
            .transpose()?,
        id: row.try_get("id")?,
        device_ref: serde_json::from_str(row.try_get("device_ref_json")?)?,
        device_code: row
            .try_get::<Option<String>, _>("device_code")?
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| RuntimeStoreError::InvalidStoredIdentifier)
            })
            .transpose()?,
        initiated_by: row.try_get("initiated_by")?,
        kind: row.try_get("kind")?,
        direction: row.try_get("direction")?,
        source: row.try_get("source")?,
        destination: row.try_get("destination")?,
        overwrite: row.try_get("overwrite")?,
        state,
        offset: u64::try_from(row.try_get::<i64, _>("offset")?)
            .map_err(|_| RuntimeStoreError::InvalidStoredOffset)?,
        size: u64::try_from(row.try_get::<i64, _>("size")?)
            .map_err(|_| RuntimeStoreError::InvalidStoredOffset)?,
        started_at_unix_ms: row.try_get("started_at_unix_ms")?,
        finished_at_unix_ms: row.try_get("finished_at_unix_ms")?,
        message: row.try_get("message")?,
        execution_observation,
        transfer_phase: phase,
        filesystem_mutation: row
            .try_get::<Option<String>, _>("filesystem_mutation")
            .unwrap_or(None)
            .map(|s| serde_json::from_str(&s))
            .transpose()?,
    })
}

pub(super) fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
