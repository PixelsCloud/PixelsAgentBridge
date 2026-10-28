//! Original operation and native identity are admitted atomically, never
//! reconstructed from a current path or from a replay on a new connection.
use super::{TaskStore, TaskStoreError};
#[cfg(any(unix, test))]
use pab_protocol::OperatorRef;
use pab_protocol::{
    ExecutionContext, FileManagerOperation, FileManagerRequest, FileManagerResult,
    FileSystemAction, FileSystemReply, FileSystemRequest, RequestId,
};
use sha2::{Digest, Sha256};
use sqlx::Row;

pub(crate) struct FilesystemExecution {
    pub request: FileSystemRequest,
    pub context: Option<ExecutionContext>,
    #[cfg(any(unix, test))]
    pub creation_receipt_pending: bool,
}
fn decode(
    row: &sqlx::sqlite::SqliteRow,
    id: RequestId,
) -> Result<Option<FilesystemExecution>, TaskStoreError> {
    let Some(data) = row.try_get::<Option<&str>, _>("request_json")? else {
        return Ok(None);
    };
    let request: FileSystemRequest = serde_json::from_str(data)?;
    if request.request_id != id
        || request.validate().is_err()
        || format!("{:x}", Sha256::digest(serde_json::to_vec(&request)?))
            != row.try_get::<String, _>("fingerprint")?
    {
        return Err(TaskStoreError::RequestConflict);
    }
    let context = row
        .try_get::<Option<&str>, _>("context_json")?
        .map(serde_json::from_str)
        .transpose()?;
    let pending: i64 = row.try_get("creation_receipt_pending")?;
    if !matches!(pending, 0 | 1) {
        return Err(TaskStoreError::RequestConflict);
    }
    Ok(Some(FilesystemExecution {
        request,
        context,
        #[cfg(any(unix, test))]
        creation_receipt_pending: pending == 1,
    }))
}
fn validate(
    execution: &FilesystemExecution,
    reply: &FileSystemReply,
) -> Result<(), TaskStoreError> {
    let request = &execution.request;
    let FileSystemAction::Managed {
        operation:
            operation @ FileManagerOperation::PrepareDirectory {
                existing_object: None,
                ..
            },
        expected_execution,
    } = &request.operation
    else {
        return Err(TaskStoreError::RequestConflict);
    };
    let expected = FileSystemReply::pending(request);
    if reply.request_id != expected.request_id
        || reply.path != expected.path
        || reply.kind != expected.kind
        || reply.destination != expected.destination
        || reply.state != "completed"
        || reply.error.is_some()
        || reply.data_size != 0
        || reply.data_sha256.is_some()
        || reply.execution_context != execution.context
        || execution.context.is_none()
    {
        return Err(TaskStoreError::RequestConflict);
    }
    let managed = reply
        .file_manager
        .as_ref()
        .ok_or(TaskStoreError::RequestConflict)?;
    if managed.execution_context != execution.context
        || !matches!(
            managed.result,
            FileManagerResult::DirectoryPrepared { created: true, .. }
        )
    {
        return Err(TaskStoreError::RequestConflict);
    }
    managed
        .validate_for(&FileManagerRequest {
            schema_version: pab_protocol::FILE_MANAGER_SCHEMA_VERSION,
            request_id: request.request_id,
            execution: request.execution,
            expected_execution: expected_execution.clone(),
            operation: operation.clone(),
        })
        .map_err(|_| TaskStoreError::RequestConflict)
}
fn same_fact(original: &FileSystemReply, reply: &FileSystemReply) -> bool {
    original.state == "completed"
        && original.error.is_none()
        && original.request_id == reply.request_id
        && original.path == reply.path
        && original.kind == reply.kind
        && original.destination == reply.destination
        && original.execution_context == reply.execution_context
        && original.file_manager == reply.file_manager
}
impl TaskStore {
    pub async fn finish_directory_metadata(
        &self,
        reply: &FileSystemReply,
    ) -> Result<(), TaskStoreError> {
        self.finish_filesystem(reply).await?;
        let row=sqlx::query("SELECT o.state,f.reply_json FROM read_operations o JOIN filesystem_results f ON f.request_id=o.request_id WHERE o.request_id=?").bind(reply.request_id.to_string()).fetch_optional(&self.pool).await?.ok_or(TaskStoreError::NotFound)?;
        let original: FileSystemReply = serde_json::from_str(row.try_get("reply_json")?)?;
        if reply.state != "completed"
            || reply.error.is_some()
            || row.try_get::<&str, _>("state")? != "completed"
            || !same_fact(&original, reply)
        {
            return Err(TaskStoreError::RequestConflict);
        }
        Ok(())
    }
    #[cfg(any(unix, test))]
    pub async fn filesystem_execution(
        &self,
        actor: OperatorRef,
        id: RequestId,
    ) -> Result<Option<FilesystemExecution>, TaskStoreError> {
        let row=sqlx::query("SELECT o.initiated_by_json,f.fingerprint,e.request_json,e.context_json,e.creation_receipt_pending FROM read_operations o JOIN filesystem_results f ON f.request_id=o.request_id LEFT JOIN filesystem_execution e ON e.request_id=o.request_id WHERE o.request_id=?").bind(id.to_string()).fetch_optional(&self.pool).await?.ok_or(TaskStoreError::NotFound)?;
        if serde_json::from_str::<OperatorRef>(row.try_get("initiated_by_json")?)? != actor {
            return Err(TaskStoreError::NotFound);
        }
        decode(&row, id)
    }
    /// Called only after an authenticated original-task lookup or a validated
    /// native worker reply. The database independently checks the exact request,
    /// frozen context and terminal fact before saving or releasing anything.
    pub async fn directory_creation_receipt(
        &self,
        reply: &FileSystemReply,
        released: bool,
    ) -> Result<(), TaskStoreError> {
        let mut tx = self.write_transaction().await?;
        let row=sqlx::query("SELECT o.state,f.fingerprint,f.reply_json,e.request_json,e.context_json,e.creation_receipt_pending FROM read_operations o JOIN filesystem_results f ON f.request_id=o.request_id LEFT JOIN filesystem_execution e ON e.request_id=o.request_id WHERE o.request_id=?").bind(reply.request_id.to_string()).fetch_optional(&mut *tx).await?.ok_or(TaskStoreError::NotFound)?;
        let execution = decode(&row, reply.request_id)?.ok_or(TaskStoreError::RequestConflict)?;
        validate(&execution, reply)?;
        let state: &str = row.try_get("state")?;
        if state == "completed" {
            let original: FileSystemReply =
                serde_json::from_str(row.try_get::<&str, _>("reply_json")?)?;
            if !same_fact(&original, reply) {
                return Err(TaskStoreError::RequestConflict);
            }
        } else {
            if released || !matches!(state, "committing" | "unconfirmed") {
                return Err(TaskStoreError::RequestConflict);
            }
            let changed=sqlx::query("UPDATE read_operations SET state='completed',finished_at_unix_ms=?,message=NULL,result_count=0 WHERE request_id=? AND state IN ('committing','unconfirmed')").bind(super::operation::now_unix_ms()).bind(reply.request_id.to_string()).execute(&mut *tx).await?;
            if changed.rows_affected() != 1 {
                return Err(TaskStoreError::RequestConflict);
            }
            let changed =
                sqlx::query("UPDATE filesystem_results SET reply_json=? WHERE request_id=?")
                    .bind(serde_json::to_string(reply)?)
                    .bind(reply.request_id.to_string())
                    .execute(&mut *tx)
                    .await?;
            if changed.rows_affected() != 1 {
                return Err(TaskStoreError::RequestConflict);
            }
        }
        if released {
            let changed = sqlx::query(
                "UPDATE filesystem_execution SET creation_receipt_pending=0 WHERE request_id=?",
            )
            .bind(reply.request_id.to_string())
            .execute(&mut *tx)
            .await?;
            if changed.rows_affected() != 1 {
                return Err(TaskStoreError::RequestConflict);
            }
        }
        tx.commit().await?;
        Ok(())
    }
}
