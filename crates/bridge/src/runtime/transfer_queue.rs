//! Durable submission metadata layered over the existing operation records.
use std::path::Path;

use pab_protocol::{DeviceCode, DeviceRef, RequestId, TransferSnapshot};
use serde::Serialize;
use sqlx::Row;

use super::{
    LocalTaskRecord, OperationRecord, RuntimeStore, RuntimeStoreError, operation::now_unix_ms,
};

#[derive(Debug, Clone)]
pub struct TransferRequest {
    pub request_id: RequestId,
    pub device_ref: DeviceRef,
    pub device_code: DeviceCode,
    pub direction: String,
    pub source: String,
    pub destination: String,
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueuedTransfer {
    #[serde(flatten)]
    pub operation: OperationRecord,
    pub phase: String,
    pub updated_at_unix_ms: i64,
    pub sha256: Option<String>,
    pub owned: bool,
}

#[derive(Clone)]
pub struct TransferQueue {
    pub(super) store: RuntimeStore,
    pub(super) session_id: String,
    pub(super) initiated_by: String,
}

impl TransferQueue {
    pub async fn remembered_device(
        &self,
        code: DeviceCode,
    ) -> Result<super::RememberedDevice, RuntimeStoreError> {
        self.store
            .remembered_devices()
            .await?
            .into_iter()
            .find(|d| d.code == code)
            .ok_or(RuntimeStoreError::NotFound)
    }

    pub async fn reconcile(
        &self,
        record: &QueuedTransfer,
        remote: &TransferSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        let operation = &record.operation;
        if remote.request_id.to_string() != operation.id
            || remote.direction
                != if operation.direction == "upload" {
                    "receive"
                } else {
                    "send"
                }
            || remote.path.as_str()
                != if operation.direction == "upload" {
                    operation.destination.as_str()
                } else {
                    operation.source.as_str()
                }
        {
            return Err(RuntimeStoreError::RequestConflict);
        }
        let id = remote.request_id;
        if remote.state == "completed"
            && remote.offset == remote.size
            && remote.finished_at_unix_ms.is_some()
        {
            let Some(digest) = remote.sha256.as_deref() else {
                return Ok(());
            };
            if record
                .sha256
                .as_deref()
                .is_some_and(|expected| expected != digest)
            {
                return Err(RuntimeStoreError::RequestConflict);
            }
            if operation.direction == "download"
                && !super::reconciliation::published_file_matches(
                    Path::new(&operation.destination),
                    remote.size,
                    digest,
                )
                .await
            {
                return Ok(());
            }
            self.progress(id, remote.offset, remote.size).await?;
            self.phase(id, "completed", Some(digest)).await?;
            self.finish(id, "completed", None).await?;
        } else if operation.direction == "upload"
            && remote.state == "failed"
            && (remote.published == Some(false) || remote.offset < remote.size)
        {
            self.finish(
                id,
                if operation.state == "cancel_requested" && remote.published == Some(false) {
                    "cancelled"
                } else {
                    "failed"
                },
                remote.message.as_deref(),
            )
            .await?;
        }
        Ok(())
    }

    pub async fn open(
        path: &Path,
        session_id: String,
        initiated_by: String,
    ) -> Result<Self, RuntimeStoreError> {
        let store = RuntimeStore::open(path).await?;
        store.start_session(&session_id).await?;
        Ok(Self {
            store,
            session_id,
            initiated_by,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn initiated_by(&self) -> &str {
        &self.initiated_by
    }

    pub async fn active_count(&self) -> Result<i64, RuntimeStoreError> {
        Ok(sqlx::query_scalar("SELECT COUNT(*) FROM runtime_operations WHERE owner_session_id = ? AND kind = 'file_transfer' AND finished_at_unix_ms IS NULL")
            .bind(&self.session_id).fetch_one(&self.store.pool).await?)
    }

    pub async fn active_for_device(&self, code: DeviceCode) -> Result<i64, RuntimeStoreError> {
        Ok(sqlx::query_scalar("SELECT COUNT(*) FROM runtime_operations WHERE owner_session_id = ? AND device_code = ? AND kind IN ('file_transfer', 'file_stat', 'file_read', 'file_write', 'file_patch', 'file_search', 'file_hash', 'mkdir', 'file_copy', 'file_move', 'file_delete', 'archive_create', 'archive_extract', 'system_info', 'disks', 'processes', 'process', 'network_interfaces') AND finished_at_unix_ms IS NULL")
            .bind(&self.session_id).bind(code.to_string()).fetch_one(&self.store.pool).await?)
    }

    pub async fn heartbeat(&self) -> Result<(), RuntimeStoreError> {
        self.store.heartbeat_session(&self.session_id).await
    }

    pub async fn close(&self) -> Result<(), RuntimeStoreError> {
        self.store.stop_session(&self.session_id).await
    }

    /// Claims the request atomically. A repeated request never launches a second worker.
    pub async fn submit(
        &self,
        spec: &TransferRequest,
    ) -> Result<(QueuedTransfer, bool), RuntimeStoreError> {
        if !matches!(spec.direction.as_str(), "upload" | "download") {
            return Err(RuntimeStoreError::RequestConflict);
        }
        let mut tx = self.store.pool.begin_with("BEGIN IMMEDIATE").await?;
        let previous = sqlx::query("SELECT * FROM runtime_operations WHERE id = ?")
            .bind(spec.request_id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
        if let Some(row) = previous {
            let matches = row.try_get::<String, _>("device_ref_json")?
                == serde_json::to_string(&spec.device_ref)?
                && row.try_get::<Option<String>, _>("device_code")?.as_deref()
                    == Some(spec.device_code.to_string().as_str())
                && row.try_get::<String, _>("initiated_by")? == self.initiated_by
                && row.try_get::<String, _>("kind")? == "file_transfer"
                && row.try_get::<String, _>("direction")? == spec.direction
                && row.try_get::<String, _>("source")? == spec.source
                && row.try_get::<String, _>("destination")? == spec.destination
                && row.try_get::<bool, _>("overwrite")? == spec.overwrite;
            if !matches {
                return Err(RuntimeStoreError::RequestConflict);
            }
            tx.commit().await?;
            return Ok((self.get(spec.request_id, spec.device_code).await?, false));
        }
        let existing_task: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM runtime_tasks WHERE request_id = ?")
                .bind(spec.request_id.to_string())
                .fetch_one(&mut *tx)
                .await?;
        if existing_task != 0 {
            return Err(RuntimeStoreError::RequestConflict);
        }
        sqlx::query("INSERT INTO runtime_operations (id, device_ref_json, device_code, initiated_by, kind, direction, source, destination, overwrite, state, started_at_unix_ms, owner_session_id) VALUES (?, ?, ?, ?, 'file_transfer', ?, ?, ?, ?, 'running', ?, ?)")
            .bind(spec.request_id.to_string()).bind(serde_json::to_string(&spec.device_ref)?)
            .bind(spec.device_code.to_string()).bind(&self.initiated_by).bind(&spec.direction)
            .bind(&spec.source).bind(&spec.destination).bind(spec.overwrite)
            .bind(now_unix_ms()).bind(&self.session_id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO runtime_async_transfers (id, phase, updated_at_unix_ms) VALUES (?, 'queued', ?)")
            .bind(spec.request_id.to_string()).bind(now_unix_ms()).execute(&mut *tx).await?;
        if spec.direction == "download" {
            let destination = Path::new(&spec.destination);
            let parent = tokio::fs::canonicalize(
                destination
                    .parent()
                    .ok_or(RuntimeStoreError::RequestConflict)?,
            )
            .await
            .map_err(RuntimeStoreError::Io)?;
            let key = parent
                .join(
                    destination
                        .file_name()
                        .ok_or(RuntimeStoreError::RequestConflict)?,
                )
                .to_string_lossy()
                .into_owned();
            #[cfg(windows)]
            let key = key.to_lowercase();
            let result = sqlx::query("INSERT INTO runtime_download_claims (destination_key, operation_id) VALUES (?, ?) ON CONFLICT(destination_key) DO NOTHING")
                .bind(key).bind(spec.request_id.to_string()).execute(&mut *tx).await?;
            if result.rows_affected() != 1 {
                return Err(RuntimeStoreError::RequestConflict);
            }
        }
        tx.commit().await?;
        Ok((self.get(spec.request_id, spec.device_code).await?, true))
    }

    pub async fn get(
        &self,
        id: RequestId,
        code: DeviceCode,
    ) -> Result<QueuedTransfer, RuntimeStoreError> {
        let row = sqlx::query("SELECT o.*, s.heartbeat_at_unix_ms, s.stopped_at_unix_ms, a.phase, a.updated_at_unix_ms, a.sha256 FROM runtime_operations o LEFT JOIN runtime_sessions s ON s.id = o.owner_session_id JOIN runtime_async_transfers a ON a.id = o.id WHERE o.id = ? AND o.device_code = ? AND o.initiated_by = ?")
            .bind(id.to_string()).bind(code.to_string()).bind(&self.initiated_by)
            .fetch_optional(&self.store.pool).await?.ok_or(RuntimeStoreError::NotFound)?;
        let owner: String = row.try_get("owner_session_id")?;
        let owned = owner == self.session_id;
        // Historic requests remain readable after their owner exits. Another live
        // process's activity is not exposed through this MCP's operation API.
        if !owned
            && row
                .try_get::<Option<i64>, _>("stopped_at_unix_ms")?
                .is_none()
            && row
                .try_get::<Option<i64>, _>("heartbeat_at_unix_ms")?
                .is_some_and(|at| at >= now_unix_ms() - 15_000)
        {
            return Err(RuntimeStoreError::NotFound);
        }
        let phase = row.try_get("phase")?;
        let updated_at_unix_ms = row.try_get("updated_at_unix_ms")?;
        let sha256 = row.try_get("sha256")?;
        let operation = super::operation::decode_operation(row, now_unix_ms() - 15_000)?;
        Ok(QueuedTransfer {
            operation,
            phase,
            updated_at_unix_ms,
            sha256,
            owned,
        })
    }

    pub async fn page(
        &self,
        code: Option<DeviceCode>,
        state: Option<&str>,
        before: Option<(i64, &str)>,
        limit: u32,
    ) -> Result<Vec<QueuedTransfer>, RuntimeStoreError> {
        let ids = sqlx::query("SELECT o.id, o.device_code FROM runtime_operations o JOIN runtime_async_transfers a ON a.id = o.id WHERE o.owner_session_id = ? AND (? IS NULL OR o.device_code = ?) AND (? IS NULL OR o.state = ?) AND (? IS NULL OR (o.started_at_unix_ms, o.id) < (?, ?)) ORDER BY o.started_at_unix_ms DESC, o.id DESC LIMIT ?")
            .bind(&self.session_id).bind(code.map(|c| c.to_string())).bind(code.map(|c| c.to_string()))
            .bind(state).bind(state).bind(before.map(|c| c.0)).bind(before.map(|c| c.0)).bind(before.map(|c| c.1))
            .bind(i64::from(limit.min(101))).fetch_all(&self.store.pool).await?;
        let mut results = Vec::with_capacity(ids.len());
        for row in ids {
            let id: String = row.try_get("id")?;
            let code: String = row.try_get("device_code")?;
            results.push(
                self.get(
                    id.parse()
                        .map_err(|_| RuntimeStoreError::InvalidStoredIdentifier)?,
                    code.parse()
                        .map_err(|_| RuntimeStoreError::InvalidStoredIdentifier)?,
                )
                .await?,
            );
        }
        Ok(results)
    }

    pub async fn phase(
        &self,
        id: RequestId,
        phase: &str,
        sha256: Option<&str>,
    ) -> Result<(), RuntimeStoreError> {
        sqlx::query("UPDATE runtime_async_transfers SET phase = ?, sha256 = COALESCE(?, sha256), updated_at_unix_ms = ? WHERE id = ? AND EXISTS (SELECT 1 FROM runtime_operations o WHERE o.id = runtime_async_transfers.id AND o.owner_session_id = ? AND o.state IN ('running', 'cancel_requested'))")
            .bind(phase).bind(sha256).bind(now_unix_ms()).bind(id.to_string()).bind(&self.session_id)
            .execute(&self.store.pool).await?;
        Ok(())
    }

    pub async fn progress(
        &self,
        id: RequestId,
        offset: u64,
        size: u64,
    ) -> Result<(), RuntimeStoreError> {
        self.store.operation_progress(id, offset, size).await
    }

    pub async fn note(&self, id: RequestId, message: &str) -> Result<(), RuntimeStoreError> {
        // Keep an unresolved error visible without pretending it is terminal.
        let message: String = message.chars().take(4096).collect();
        sqlx::query("UPDATE runtime_operations SET message = ? WHERE id = ? AND owner_session_id = ? AND state IN ('running', 'cancel_requested')")
            .bind(message).bind(id.to_string()).bind(&self.session_id).execute(&self.store.pool).await?;
        Ok(())
    }

    pub async fn finish(
        &self,
        id: RequestId,
        state: &str,
        message: Option<&str>,
    ) -> Result<(), RuntimeStoreError> {
        self.store.finish_operation(id, state, message).await?;
        Ok(())
    }

    pub async fn cancel(
        &self,
        id: RequestId,
        code: DeviceCode,
    ) -> Result<QueuedTransfer, RuntimeStoreError> {
        let record = self.get(id, code).await?;
        if !record.owned {
            return Err(RuntimeStoreError::RequestConflict);
        }
        self.store
            .request_transfer_cancellation(id, &self.session_id)
            .await?;
        self.get(id, code).await
    }

    pub async fn track_task(&self, id: RequestId) -> Result<(), RuntimeStoreError> {
        self.store
            .track_task_owner(id, &self.session_id, &self.initiated_by)
            .await
    }

    pub async fn command(
        &self,
        id: RequestId,
        code: DeviceCode,
    ) -> Result<LocalTaskRecord, RuntimeStoreError> {
        let visible: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runtime_task_owners o JOIN runtime_sessions s ON s.id = o.owner_session_id WHERE o.request_id = ? AND o.initiated_by = ? AND (o.owner_session_id = ? OR s.stopped_at_unix_ms IS NOT NULL OR s.heartbeat_at_unix_ms < ?)")
            .bind(id.to_string()).bind(&self.initiated_by).bind(&self.session_id).bind(now_unix_ms()-15_000).fetch_one(&self.store.pool).await?;
        if visible != 1 {
            return Err(RuntimeStoreError::NotFound);
        }
        let task = self.store.get_by_request(id).await?;
        if !self
            .store
            .remembered_devices()
            .await?
            .iter()
            .any(|d| d.code == code && d.device_ref == task.device_ref)
        {
            return Err(RuntimeStoreError::NotFound);
        }
        Ok(task)
    }

    pub async fn owns_task(&self, id: RequestId) -> Result<bool, RuntimeStoreError> {
        let owner: Option<String> = sqlx::query_scalar(
            "SELECT owner_session_id FROM runtime_task_owners WHERE request_id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.store.pool)
        .await?;
        Ok(owner.as_deref() == Some(&self.session_id))
    }

    pub async fn active_filesystems(
        &self,
    ) -> Result<Vec<(RequestId, DeviceCode)>, RuntimeStoreError> {
        let rows = sqlx::query("SELECT o.id, o.device_code FROM runtime_operations o WHERE (EXISTS (SELECT 1 FROM runtime_filesystem_results f WHERE f.id=o.id) OR EXISTS (SELECT 1 FROM runtime_system_results q WHERE q.id=o.id)) AND o.owner_session_id = ? AND o.initiated_by = ? AND o.finished_at_unix_ms IS NULL ORDER BY o.started_at_unix_ms LIMIT 100").bind(&self.session_id).bind(&self.initiated_by).fetch_all(&self.store.pool).await?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.try_get::<String, _>("id")?
                        .parse()
                        .map_err(|_| RuntimeStoreError::RequestConflict)?,
                    row.try_get::<String, _>("device_code")?
                        .parse()
                        .map_err(|_| RuntimeStoreError::RequestConflict)?,
                ))
            })
            .collect()
    }

    pub async fn owns_filesystem(&self, id: RequestId) -> Result<bool, RuntimeStoreError> {
        self.store.owns_filesystem(id, &self.session_id).await
    }

    pub async fn filesystem_record(
        &self,
        id: RequestId,
        code: DeviceCode,
    ) -> Result<(pab_protocol::DeviceRef, pab_protocol::FileSystemReply), RuntimeStoreError> {
        self.store
            .filesystem_record(id, Some(code), &self.initiated_by, &self.session_id)
            .await
    }

    pub async fn operation_page(
        &self,
        code: Option<DeviceCode>,
        state: Option<&str>,
        before: Option<(i64, &str)>,
        limit: u32,
    ) -> Result<Vec<serde_json::Value>, RuntimeStoreError> {
        let rows = sqlx::query("WITH entries AS (
            SELECT o.id, o.device_code, 'file_transfer' AS kind, o.state, o.started_at_unix_ms AS started, o.finished_at_unix_ms AS finished, o.offset, o.size, a.phase, a.updated_at_unix_ms AS updated, o.message, o.source, o.destination, NULL AS partial
            FROM runtime_operations o JOIN runtime_async_transfers a ON a.id = o.id WHERE o.owner_session_id = ?
            UNION ALL
            SELECT o.id, o.device_code, o.kind, o.state, o.started_at_unix_ms, o.finished_at_unix_ms, o.offset, o.size, json_extract(f.reply_json, '$.state'), json_extract(f.reply_json, '$.progress.updated_at_unix_ms'), o.message, o.source, o.destination, json_extract(f.reply_json, '$.mutation.partial')
            FROM runtime_operations o JOIN runtime_filesystem_results f ON f.id = o.id WHERE o.owner_session_id = ?
            UNION ALL
            SELECT o.id,o.device_code,o.kind,o.state,o.started_at_unix_ms,o.finished_at_unix_ms,o.offset,o.size,json_extract(q.reply_json,'$.state'),json_extract(q.reply_json,'$.sampled_at_unix_ms'),o.message,o.source,o.destination,NULL
            FROM runtime_operations o JOIN runtime_system_results q ON q.id=o.id WHERE o.owner_session_id=?
            UNION ALL
            SELECT t.request_id, d.device_code, 'command',
                CASE json_extract(t.snapshot_json, '$.state') WHEN 'succeeded' THEN 'completed' WHEN 'interrupted' THEN 'failed' WHEN 'accepted' THEN 'running' ELSE COALESCE(json_extract(t.snapshot_json, '$.state'), 'running') END,
                own.created_at_unix_ms, json_extract(t.snapshot_json, '$.finished_at_unix_ms'), 0, 0,
                COALESCE(json_extract(t.snapshot_json, '$.state'), 'queued'), NULL, NULL, NULL, NULL, NULL
            FROM runtime_tasks t JOIN runtime_task_owners own ON own.request_id = t.request_id LEFT JOIN remembered_devices d ON d.device_ref_json = t.device_ref_json WHERE own.owner_session_id = ?
        ) SELECT * FROM entries WHERE (? IS NULL OR device_code = ?) AND (? IS NULL OR state = ?) AND (? IS NULL OR (started, id) < (?, ?)) ORDER BY started DESC, id DESC LIMIT ?")
            .bind(&self.session_id).bind(&self.session_id).bind(&self.session_id).bind(&self.session_id).bind(code.map(|c| c.to_string())).bind(code.map(|c| c.to_string()))
            .bind(state).bind(state).bind(before.map(|c| c.0)).bind(before.map(|c| c.0)).bind(before.map(|c| c.1))
            .bind(i64::from(limit.min(101))).fetch_all(&self.store.pool).await?;
        rows.into_iter().map(|row| Ok(serde_json::json!({
            "operation_id": row.try_get::<String, _>("id")?, "device_code": row.try_get::<Option<String>, _>("device_code")?,
            "kind": row.try_get::<String, _>("kind")?, "state": row.try_get::<String, _>("state")?,
            "started_at_unix_ms": row.try_get::<i64, _>("started")?, "finished_at_unix_ms": row.try_get::<Option<i64>, _>("finished")?,
            "offset": row.try_get::<i64, _>("offset")?, "size": row.try_get::<i64, _>("size")?,
            "phase": row.try_get::<String, _>("phase")?, "updated_at_unix_ms": row.try_get::<Option<i64>, _>("updated")?,
            "message": row.try_get::<Option<String>, _>("message")?,
            "source": row.try_get::<Option<String>, _>("source")?, "destination": row.try_get::<Option<String>, _>("destination")?,
            "partial": row.try_get::<Option<i64>, _>("partial")?.map(|v| v != 0)
        }))).collect()
    }
}

impl RuntimeStore {
    pub(super) async fn track_task_owner(
        &self,
        id: RequestId,
        owner: &str,
        actor: &str,
    ) -> Result<(), RuntimeStoreError> {
        sqlx::query("INSERT INTO runtime_task_owners (request_id, owner_session_id, initiated_by, created_at_unix_ms) VALUES (?, ?, ?, ?) ON CONFLICT(request_id) DO NOTHING")
            .bind(id.to_string()).bind(owner).bind(actor).bind(now_unix_ms()).execute(&self.pool).await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "transfer_queue_tests.rs"]
mod tests;
