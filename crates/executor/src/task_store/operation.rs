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
            "SELECT o.initiated_by_json, o.direction, o.path, o.state, o.offset, o.size, o.sha256, o.finished_at_unix_ms, o.message, e.context_json, e.request_json, p.committed_at_unix_ms, m.result_json FROM transfer_operations o LEFT JOIN transfer_execution e ON e.request_id=o.request_id LEFT JOIN transfer_publications p ON p.request_id=o.request_id LEFT JOIN transfer_metadata_results m ON m.request_id=o.request_id WHERE o.request_id = ?",
        )
        .bind(request_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(TaskStoreError::NotFound)?;
        let stored_actor: OperatorRef = serde_json::from_str(row.try_get("initiated_by_json")?)?;
        if stored_actor != initiated_by {
            return Err(TaskStoreError::NotFound);
        }
        let content_published=row.try_get::<Option<i64>,_>("committed_at_unix_ms")?.is_some();
        // Content publication and attribute finalization are separate facts.
        // A matching hash (or even a durable rename) cannot prove attributes.
        let requested=row.try_get::<Option<&str>,_>("request_json")?
            .map(serde_json::from_str::<pab_protocol::FileTransferRequest>).transpose()?
            .and_then(|request|request.metadata);
        let metadata_result:Option<pab_protocol::FileTransferMetadataResult>=row.try_get::<Option<&str>,_>("result_json")?.map(serde_json::from_str).transpose()?;
        if let Some(result)=&metadata_result {
            result.validate_for(requested.as_ref().ok_or(TaskStoreError::RequestConflict)?).map_err(|_|TaskStoreError::RequestConflict)?;
            if !content_published {return Err(TaskStoreError::RequestConflict);}
        }
        let attributes_pending=requested.is_some() && metadata_result.is_none();
        let mut snapshot = TransferSnapshot {
            metadata_result,
            initiating_user: self.initiating_user(initiated_by, request_id).await?,
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
            published: if content_published {Some(true)} else {match (
                row.try_get::<String, _>("direction")?.as_str(),
                row.try_get::<String, _>("state")?.as_str(),
            ) {
                ("receive", "completed") => Some(true),
                ("receive", "failed" | "interrupted" | "cancelled" | "paused" | "stopping") => Some(false),
                _ => None,
            }},
        };
        // A process can stop between publishing a verified file and recording
        // completion. Resolve that window using file evidence, never by replay.
        if snapshot.direction == "receive"
            && snapshot.state == "committing"
            && !attributes_pending
            && (content_published || (snapshot.execution_context.as_ref().is_none_or(|context| {
                context
                    .identity
                    .as_ref()
                    .is_none_or(|identity| identity.mode == pab_protocol::ExecutionMode::Service)
            })
            && publication_matches(&snapshot, row.try_get::<Option<&str>, _>("request_json")?).await))
        {
            self.finish_transfer(request_id, "completed", None).await?;
            // Return the persisted fact, including its exact timestamp. Taking
            // the clock again creates two different terminal receipts for one
            // request, and a concurrent publication failure may win the UPDATE.
            let settled=sqlx::query("SELECT state,finished_at_unix_ms,message FROM transfer_operations WHERE request_id=?")
                .bind(request_id.to_string()).fetch_one(&self.pool).await?;
            snapshot.state=settled.try_get("state")?;
            snapshot.finished_at_unix_ms=settled.try_get("finished_at_unix_ms")?;
            snapshot.message=settled.try_get("message")?;
            snapshot.published=if content_published || snapshot.state=="completed" {Some(true)}else if matches!(snapshot.state.as_str(),"failed"|"interrupted"|"cancelled"|"paused"|"stopping") {Some(false)}else{None};
        }
        Ok(snapshot)
    }

    pub async fn interrupt_transfers(&self) -> Result<(), TaskStoreError> {
        sqlx::query(
            "UPDATE transfer_operations SET state = 'interrupted', finished_at_unix_ms = ?, message = 'Executor stopped during transfer' WHERE state IN ('running','stopping')",
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
        sqlx::query("UPDATE transfer_operations SET state = 'failed', message = ?, finished_at_unix_ms = ? WHERE request_id = ? AND state = 'committing' AND NOT EXISTS(SELECT 1 FROM transfer_publications p WHERE p.request_id=transfer_operations.request_id)")
            .bind(message).bind(now_unix_ms()).bind(id.to_string()).execute(&self.pool).await?;
        Ok(())
    }

    /// Only the recorder bound to the accepted request may report a successful
    /// rename. Persist it before final permissions can prevent reopening the file.
    pub(crate) async fn confirm_transfer_publication(&self,id:RequestId)->Result<(),TaskStoreError> {
        let result=sqlx::query("INSERT INTO transfer_publications(request_id,committed_at_unix_ms) SELECT request_id,? FROM transfer_operations WHERE request_id=? AND direction='receive' AND state IN ('committing','completed') AND offset=size AND sha256 IS NOT NULL AND length(sha256)=64 AND sha256 NOT GLOB '*[^0-9a-f]*' ON CONFLICT(request_id) DO UPDATE SET committed_at_unix_ms=transfer_publications.committed_at_unix_ms")
            .bind(now_unix_ms()).bind(id.to_string()).execute(&self.pool).await?;
        if result.rows_affected()!=1 {return Err(TaskStoreError::RequestConflict);}
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
            "UPDATE transfer_operations SET state = ?, message = ?, finished_at_unix_ms = ? WHERE request_id = ? AND (state = 'running' OR (state = 'committing' AND ? = 'completed')) AND (? != 'completed' OR direction != 'receive' OR NOT EXISTS(SELECT 1 FROM transfer_execution e WHERE e.request_id=transfer_operations.request_id AND json_type(e.request_json,'$.metadata')='object') OR EXISTS(SELECT 1 FROM transfer_metadata_results m WHERE m.request_id=transfer_operations.request_id))",
        )
        .bind(state)
        .bind(message)
        .bind(now_unix_ms())
        .bind(request_id.to_string())
        .bind(state)
        .bind(state)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

async fn publication_matches(snapshot: &TransferSnapshot, request_json: Option<&str>) -> bool {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncReadExt;
    let Some(expected) = snapshot.sha256.as_deref() else {
        return false;
    };
    let request = match request_json {
        Some(json)=>match serde_json::from_str::<pab_protocol::FileTransferRequest>(json) {Ok(request)=>Some(request),Err(_)=>return false},
        None=>None,
    };
    let path = match request.as_ref().and_then(|request|request.native_path.as_ref()) {
        Some(native)=>match pab_file_manager::decode_path(native) {Ok(path)=>path,Err(_)=>return false},
        None=>std::path::PathBuf::from(&snapshot.path),
    };
    let parent=request.and_then(|request|request.destination_parent);
    let Ok(Ok(opened))=tokio::task::spawn_blocking(move ||pab_file_manager::open_source_in_parent(&path,parent.as_ref().map(|value|value.identity.as_str()))).await else {
        return false;
    };
    let mut file=tokio::fs::File::from_std(opened);
    let Ok(metadata)=file.metadata().await else {return false;};
    if !metadata.is_file() || metadata.len() != snapshot.size {
        return false;
    }
    let mut digest = Sha256::new();
    // This recovery future is embedded in history/acceptance callers, even
    // while they are not reconciling. Keep the streaming buffer off their stacks.
    let mut buffer = vec![0u8;64*1024];
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
