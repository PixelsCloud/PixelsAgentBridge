use pab_protocol::{FileSystemReply, FileSystemRequest, OperatorRef, RequestId};
use sqlx::Row;

use super::{TaskStore, TaskStoreError};

impl TaskStore {
    pub async fn update_filesystem_progress(
        &self,
        reply: &FileSystemReply,
    ) -> Result<(), TaskStoreError> {
        // Cancellation lives in the operation row; a late progress write cannot erase it.
        sqlx::query("UPDATE filesystem_results SET reply_json = ? WHERE request_id = ? AND EXISTS (SELECT 1 FROM read_operations o WHERE o.request_id = filesystem_results.request_id AND o.state IN ('running', 'cancel_requested'))")
            .bind(serde_json::to_string(reply)?).bind(reply.request_id.to_string()).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn request_filesystem_cancel(
        &self,
        reply: &FileSystemReply,
    ) -> Result<(), TaskStoreError> {
        sqlx::query("UPDATE read_operations SET state = 'cancel_requested' WHERE request_id = ? AND state = 'running' AND kind IN ('file_hash', 'file_copy', 'file_move', 'file_delete', 'archive_create', 'archive_extract')")
            .bind(reply.request_id.to_string()).execute(&self.pool).await?;
        Ok(())
    }

    #[cfg(test)]
    pub async fn accept_filesystem(
        &self,
        actor: OperatorRef,
        request: &FileSystemRequest,
        fingerprint: &str,
    ) -> Result<Option<FileSystemReply>, TaskStoreError> {
        self.accept_filesystem_with_context(actor, request, fingerprint, None)
            .await
    }

    pub async fn existing_filesystem(
        &self,
        actor: OperatorRef,
        request: &FileSystemRequest,
        fingerprint: &str,
    ) -> Result<Option<FileSystemReply>, TaskStoreError> {
        let existing = sqlx::query("SELECT o.initiated_by_json, f.fingerprint FROM read_operations o LEFT JOIN filesystem_results f ON f.request_id = o.request_id WHERE o.request_id = ?")
            .bind(request.request_id.to_string()).fetch_optional(&self.pool).await?;
        if let Some(row) = existing {
            if serde_json::from_str::<OperatorRef>(row.try_get("initiated_by_json")?)? != actor {
                return Err(TaskStoreError::NotFound);
            }
            if row.try_get::<Option<String>, _>("fingerprint")?.as_deref() != Some(fingerprint) {
                return Err(TaskStoreError::RequestConflict);
            }
            return self
                .get_filesystem(actor, request.request_id)
                .await
                .map(Some);
        }
        Ok(None)
    }

    pub async fn accept_filesystem_with_context(
        &self,
        actor: OperatorRef,
        request: &FileSystemRequest,
        fingerprint: &str,
        context: Option<&pab_protocol::ExecutionContext>,
    ) -> Result<Option<FileSystemReply>, TaskStoreError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing = sqlx::query("SELECT o.initiated_by_json, o.state, f.fingerprint, f.reply_json FROM read_operations o LEFT JOIN filesystem_results f ON f.request_id = o.request_id WHERE o.request_id = ?")
            .bind(request.request_id.to_string()).fetch_optional(&mut *tx).await?;
        if let Some(row) = existing {
            let stored: OperatorRef = serde_json::from_str(row.try_get("initiated_by_json")?)?;
            if stored != actor {
                return Err(TaskStoreError::NotFound);
            }
            if row.try_get::<Option<String>, _>("fingerprint")?.as_deref() != Some(fingerprint) {
                return Err(TaskStoreError::RequestConflict);
            }
            let mut reply = row
                .try_get::<Option<String>, _>("reply_json")?
                .map(|value| serde_json::from_str::<FileSystemReply>(&value))
                .transpose()?
                .unwrap_or_else(|| FileSystemReply::pending(request));
            let state: String = row.try_get("state")?;
            reply.state = if state == "committing" {
                "unconfirmed".to_owned()
            } else {
                state
            };
            reply.data_size = 0;
            reply.data_sha256 = None;
            tx.commit().await?;
            return Ok(Some(reply));
        }
        sqlx::query("INSERT INTO read_operations (request_id, initiated_by_json, kind, path, state, started_at_unix_ms) VALUES (?, ?, ?, ?, 'running', ?)")
            .bind(request.request_id.to_string()).bind(serde_json::to_string(&actor)?).bind(request.operation.kind()).bind(&request.path).bind(super::operation::now_unix_ms()).execute(&mut *tx).await?;
        let mut pending = FileSystemReply::pending(request);
        pending.execution_context = context.cloned();
        sqlx::query(
            "INSERT INTO filesystem_results (request_id, fingerprint, reply_json) VALUES (?, ?, ?)",
        )
        .bind(request.request_id.to_string())
        .bind(fingerprint)
        .bind(serde_json::to_string(&pending)?)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(None)
    }

    pub async fn get_filesystem(
        &self,
        actor: OperatorRef,
        id: RequestId,
    ) -> Result<FileSystemReply, TaskStoreError> {
        let row = sqlx::query("SELECT o.*, f.reply_json FROM read_operations o JOIN filesystem_results f ON f.request_id = o.request_id WHERE o.request_id = ?")
            .bind(id.to_string()).fetch_optional(&self.pool).await?.ok_or(TaskStoreError::NotFound)?;
        if serde_json::from_str::<OperatorRef>(row.try_get("initiated_by_json")?)? != actor {
            return Err(TaskStoreError::NotFound);
        }
        let mut reply = if let Some(json) = row.try_get::<Option<String>, _>("reply_json")? {
            serde_json::from_str::<FileSystemReply>(&json)?
        } else {
            FileSystemReply {
                directory: None,
                execution_context: None,
                log: None,
                patch_preview: None,
                request_id: id,
                path: row.try_get("path")?,
                kind: row.try_get("kind")?,
                state: String::new(),
                metadata: None,
                range: None,
                error: None,
                data_size: 0,
                data_sha256: None,
                search: None,
                progress: None,
                created_paths: None,
                destination: None,
                mutation: None,
            }
        };
        let state: String = row.try_get("state")?;
        reply.state = if state == "committing" {
            "unconfirmed".to_owned()
        } else {
            state
        };
        reply.data_size = 0;
        reply.data_sha256 = None;
        Ok(reply)
    }

    pub async fn begin_file_publication(
        &self,
        reply: &FileSystemReply,
    ) -> Result<(), TaskStoreError> {
        let mut tx = self.pool.begin().await?;
        let changed = sqlx::query("UPDATE read_operations SET state = 'committing' WHERE request_id = ? AND state = 'running'").bind(reply.request_id.to_string()).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(TaskStoreError::RequestConflict);
        }
        sqlx::query("UPDATE filesystem_results SET reply_json = ? WHERE request_id = ?")
            .bind(serde_json::to_string(reply)?)
            .bind(reply.request_id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn finish_filesystem(&self, reply: &FileSystemReply) -> Result<(), TaskStoreError> {
        let mut tx = self.pool.begin().await?;
        let changed = sqlx::query("UPDATE read_operations SET state = ?, finished_at_unix_ms = ?, message = ?, result_count = ? WHERE request_id = ? AND state IN ('running', 'committing', 'unconfirmed', 'cancel_requested')")
            .bind(&reply.state).bind(super::operation::now_unix_ms()).bind(reply.error.as_ref().map(|error| error.message.as_str())).bind(reply.metadata.as_ref().map_or_else(|| reply.directory.as_ref().map_or(0, |page| page.entries.len() as i64), |meta| meta.size.min(i64::MAX as u64) as i64)).bind(reply.request_id.to_string()).execute(&mut *tx).await?;
        if changed.rows_affected() == 1 {
            let mut summary = reply.clone();
            summary.data_size = 0;
            summary.data_sha256 = None;
            sqlx::query("UPDATE filesystem_results SET reply_json = ? WHERE request_id = ?")
                .bind(serde_json::to_string(&summary)?)
                .bind(reply.request_id.to_string())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
