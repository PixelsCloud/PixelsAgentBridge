use super::{OperationRecord, RuntimeStore, RuntimeStoreError, TransferQueue};
use pab_protocol::{ExecutionContext, FileTransferOptions, RequestId, TransferSnapshot};
use sqlx::Row;

impl RuntimeStore {
    pub(super) async fn transfer_context(
        &self,
        id: RequestId,
    ) -> Result<(FileTransferOptions, Option<ExecutionContext>), RuntimeStoreError> {
        let row = sqlx::query(
            "SELECT options_json,context_json FROM runtime_transfer_context WHERE id=?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok((Default::default(), None));
        };
        Ok((
            serde_json::from_str(row.try_get("options_json")?)?,
            row.try_get::<Option<String>, _>("context_json")?
                .map(|json| serde_json::from_str(&json))
                .transpose()?,
        ))
    }

    pub(super) async fn observe_transfer_context(
        &self,
        record: &OperationRecord,
        remote: &TransferSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        if remote.request_id.to_string() != record.id
            || remote.direction
                != if record.direction == "upload" {
                    "receive"
                } else {
                    "send"
                }
            || remote.path
                != if record.direction == "upload" {
                    &record.destination
                } else {
                    &record.source
                }
                .as_str()
        {
            return Err(RuntimeStoreError::RequestConflict);
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row = sqlx::query(
            "SELECT options_json,context_json FROM runtime_transfer_context WHERE id=?",
        )
        .bind(&record.id)
        .fetch_optional(&mut *tx)
        .await?;
        let (options, previous): (FileTransferOptions, Option<ExecutionContext>) =
            if let Some(row) = row {
                (
                    serde_json::from_str(row.try_get("options_json")?)?,
                    row.try_get::<Option<String>, _>("context_json")?
                        .map(|json| serde_json::from_str(&json))
                        .transpose()?,
                )
            } else {
                (Default::default(), None)
            };
        let valid_identity = if options.execution.is_service() {
            remote.execution_context.as_ref().is_none_or(|c| {
                c.identity.as_ref().is_none_or(|i| {
                    i.mode == pab_protocol::ExecutionMode::Service && i.validate().is_ok()
                })
            })
        } else {
            remote
                .execution_context
                .as_ref()
                .and_then(|c| c.identity.as_ref())
                .is_some_and(|i| {
                    i.mode == pab_protocol::ExecutionMode::User && i.validate().is_ok()
                })
        };
        if !valid_identity
            || previous
                .as_ref()
                .is_some_and(|prior| remote.execution_context.as_ref() != Some(prior))
        {
            return Err(RuntimeStoreError::SnapshotIdentityMismatch);
        }
        if let Some(original) = options.resume_from {
            let original_context: Option<String> =
                sqlx::query_scalar("SELECT context_json FROM runtime_transfer_context WHERE id=?")
                    .bind(original.to_string())
                    .fetch_optional(&mut *tx)
                    .await?
                    .flatten();
            let expected: ExecutionContext = serde_json::from_str(
                original_context
                    .as_deref()
                    .ok_or(RuntimeStoreError::SnapshotIdentityMismatch)?,
            )?;
            if remote.execution_context.as_ref() != Some(&expected) {
                return Err(RuntimeStoreError::SnapshotIdentityMismatch);
            }
        }
        if let Some(context) = &remote.execution_context {
            sqlx::query("INSERT INTO runtime_transfer_context (id,options_json,context_json) VALUES (?,?,?) ON CONFLICT(id) DO UPDATE SET context_json=excluded.context_json")
                .bind(&record.id).bind(serde_json::to_string(&options)?).bind(serde_json::to_string(context)?)
                .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

impl TransferQueue {
    pub async fn observe_acceptance(
        &self,
        id: RequestId,
        code: pab_protocol::DeviceCode,
        remote: &TransferSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        let record = self.get(id, code).await?;
        self.store
            .observe_transfer_context(&record.operation, remote)
            .await
    }
}
