use crate::{BatchStore,FileManagerError};
use pab_protocol::{ExecutionContext,FileByteCount,RequestId,TransferSnapshot};
use serde::{Deserialize,Serialize};
use sqlx::Row;

#[derive(Clone,Debug,Default,Serialize,Deserialize)]
pub struct AttemptEvidence {
    /// Persisted before any managed local download bytes are written.
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub local_staging:Option<crate::PrivateFileStagingReceipt>,
    /// An actual receiver removal, distinct from cancellation intent or a
    /// missing pathname. Confirmed removed bytes cannot be resumed or published.
    #[serde(default)]
    pub local_staging_discarded:bool,
    pub digest:Option<String>,
    pub context:Option<ExecutionContext>,
    pub receipt:Option<TransferSnapshot>,
    pub local_checkpoint:Option<FileByteCount>,
    /// Receiver-side gate written before atomic rename. Final-file recovery
    /// additionally requires an independent, no-follow size and hash check.
    #[serde(default)]
    pub local_publication_intent:bool,
    /// Actual receiver rename, distinct from permission to attempt it.
    #[serde(default)]
    pub local_published:bool,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub local_metadata_requested:Option<pab_protocol::FileTransferMetadata>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub local_metadata_result:Option<pab_protocol::FileTransferMetadataResult>,
}
impl BatchStore {
    pub async fn items_filtered(&self,owner:&str,batch:RequestId,after:u32,state:Option<pab_protocol::FileItemState>)->Result<Vec<pab_protocol::FileBatchItem>,FileManagerError> {
        let filter=state.map(|state|serde_json::to_string(&state)).transpose()?.map(|state|state.trim_matches('"').to_owned());
        let rows=sqlx::query("SELECT i.*,e.data AS attempt_evidence,df.result AS directory_outcome FROM fm_items i JOIN fm_batches b ON b.id=i.batch LEFT JOIN fm_attempt_evidence e ON e.attempt=i.attempt LEFT JOIN fm_directory_finalizations df ON df.attempt=i.attempt WHERE i.batch=? AND b.owner=? AND (? IS NULL OR i.state=?) ORDER BY i.seq LIMIT 128 OFFSET ?")
            .bind(batch.to_string()).bind(owner).bind(&filter).bind(&filter).bind(i64::from(after)).fetch_all(&self.pool).await?;
        rows.iter().map(crate::batch::decode_item).collect()
    }
    pub async fn pending_observations(&self,owner:&str,batch:RequestId)->Result<Vec<pab_protocol::FileBatchItem>,FileManagerError> {
        let rows=sqlx::query("SELECT i.* FROM fm_items i JOIN fm_batches b ON b.id=i.batch WHERE i.batch=? AND b.owner=? AND i.worker IS NULL AND i.state IN ('reconciling','unconfirmed') ORDER BY i.seq LIMIT 16")
            .bind(batch.to_string()).bind(owner).fetch_all(&self.pool).await?;
        rows.iter().map(crate::batch::decode_item).collect()
    }
    pub(crate) async fn init_attempt_evidence(&self)->Result<(),FileManagerError> {
        sqlx::query("CREATE TABLE IF NOT EXISTS fm_attempt_evidence(attempt TEXT PRIMARY KEY REFERENCES fm_attempts(id) ON DELETE CASCADE,data TEXT NOT NULL)").execute(&self.pool).await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS fm_batch_context(batch TEXT PRIMARY KEY REFERENCES fm_batches(id) ON DELETE CASCADE,context TEXT NOT NULL)").execute(&self.pool).await?;Ok(())
    }
    pub async fn freeze_batch_context(&self,owner:&str,batch:RequestId,context:&ExecutionContext)->Result<(),FileManagerError> {
        let mut tx=self.write_transaction().await?;
        let row=sqlx::query("SELECT b.spec,c.context FROM fm_batches b LEFT JOIN fm_batch_context c ON c.batch=b.id WHERE b.id=? AND b.owner=?").bind(batch.to_string()).bind(owner).fetch_optional(&mut *tx).await?.ok_or(FileManagerError::NotFound)?;
        let spec:pab_protocol::FileBatchSpec=serde_json::from_str(row.try_get("spec")?)?;
        if context.identity.as_ref().is_none_or(|identity|spec.execution.is_service()!=(identity.mode==pab_protocol::ExecutionMode::Service)) {return Err(FileManagerError::Conflict);}
        let data=serde_json::to_string(context)?;
        let identity=pab_protocol::FileExecutionIdentity::from_context(context).map_err(FileManagerError::Invalid)?;
        if let Some(previous)=row.try_get::<Option<String>,_>("context")? {
            let previous:ExecutionContext=serde_json::from_str(&previous)?;
            if !identity.matches_context(&previous){return Err(FileManagerError::Conflict);}
        }
        sqlx::query("INSERT INTO fm_batch_context(batch,context) VALUES(?,?) ON CONFLICT(batch) DO NOTHING").bind(batch.to_string()).bind(data).execute(&mut *tx).await?;
        tx.commit().await?;Ok(())
    }
    pub async fn batch_context(&self,owner:&str,batch:RequestId)->Result<Option<ExecutionContext>,FileManagerError> {
        let row=sqlx::query("SELECT c.context FROM fm_batches b LEFT JOIN fm_batch_context c ON c.batch=b.id WHERE b.id=? AND b.owner=?").bind(batch.to_string()).bind(owner).fetch_optional(&self.pool).await?.ok_or(FileManagerError::NotFound)?;
        row.try_get::<Option<String>,_>("context")?.map(|data|serde_json::from_str(&data)).transpose().map_err(Into::into)
    }
    pub async fn attempt_evidence(&self,owner:&str,attempt:RequestId)->Result<AttemptEvidence,FileManagerError> {
        let row=sqlx::query("SELECT e.data FROM fm_attempts a JOIN fm_items i ON i.id=a.item JOIN fm_batches b ON b.id=i.batch LEFT JOIN fm_attempt_evidence e ON e.attempt=a.id WHERE a.id=? AND b.owner=?")
            .bind(attempt.to_string()).bind(owner).fetch_optional(&self.pool).await?.ok_or(FileManagerError::NotFound)?;
        row.try_get::<Option<String>,_>("data")?.map(|data|serde_json::from_str(&data)).transpose().map(|data|data.unwrap_or_default()).map_err(Into::into)
    }
    /// Only the owning worker may add evidence. Hash and execution identity are
    /// immutable once observed; later receipts can advance the original fact.
    pub async fn observe_attempt(&self,owner:&str,item:RequestId,worker:RequestId,attempt:RequestId,incoming:&AttemptEvidence)->Result<(),FileManagerError> {
        let mut tx=self.write_transaction().await?;
        let row=sqlx::query("SELECT i.spec,b.spec AS batch_spec,b.intent AS batch_intent,e.data,t.data AS target_data,r.data AS parent_data,d.data AS root_data,dep.parent AS parent_item FROM fm_items i JOIN fm_batches b ON b.id=i.batch LEFT JOIN fm_attempt_evidence e ON e.attempt=i.attempt LEFT JOIN fm_item_targets t ON t.item=i.id LEFT JOIN fm_dependencies dep ON dep.child=i.id LEFT JOIN fm_directory_ready r ON r.item=dep.parent LEFT JOIN fm_batch_destinations d ON d.batch=i.batch WHERE i.id=? AND b.owner=? AND i.worker=? AND i.attempt=?")
            .bind(item.to_string()).bind(owner).bind(worker.to_string()).bind(attempt.to_string()).fetch_optional(&mut *tx).await?.ok_or(FileManagerError::NotFound)?;
        let spec:pab_protocol::FileBatchItemSpec=serde_json::from_str(row.try_get("spec")?)?;
        if incoming.local_staging.is_some() || incoming.local_staging_discarded || incoming.local_publication_intent || incoming.local_published || incoming.local_metadata_requested.is_some() || incoming.local_metadata_result.is_some() {
            let batch:pab_protocol::FileBatchSpec=serde_json::from_str(row.try_get("batch_spec")?)?;
            if batch.direction!=pab_protocol::FileTransferDirection::Download {return Err(FileManagerError::Conflict);}
        }
        let mut old:AttemptEvidence=row.try_get::<Option<String>,_>("data")?.map(|data|serde_json::from_str(&data)).transpose()?.unwrap_or_default();
        if let Some(staging)=&incoming.local_staging {
            staging.validate()?;
            let target:crate::ItemTarget=serde_json::from_str(row.try_get::<Option<&str>,_>("target_data")?.ok_or(FileManagerError::Conflict)?)?;
            let parent:crate::DirectoryReady=serde_json::from_str(row.try_get::<Option<&str>,_>(if row.try_get::<Option<&str>,_>("parent_item")?.is_some(){"parent_data"}else{"root_data"})?.ok_or(FileManagerError::Conflict)?)?;
            if staging.destination!=target.path || staging.parent_identity!=parent.identity
                || old.local_staging.as_ref().is_some_and(|previous|previous!=staging) {
                return Err(FileManagerError::Conflict);
            }
            old.local_staging=Some(staging.clone());
        }
        if incoming.local_staging_discarded {
            // Legacy Windows staging also has an original locked handle. Only
            // persisted explicit cancellation may discard it without a Unix
            // private-directory receipt, or after an intent whose commit lost.
            let cancelled=row.try_get::<&str,_>("batch_intent")?=="cancel";
            if (old.local_staging.is_none() || old.local_publication_intent) && !cancelled || old.local_published
                || incoming.local_publication_intent || incoming.local_published {
                return Err(FileManagerError::Conflict);
            }
            old.local_staging_discarded=true;
        }
        if old.local_staging_discarded && (incoming.local_publication_intent || incoming.local_published) {return Err(FileManagerError::Conflict);}
        if let Some(digest)=&incoming.digest {
            if !pab_protocol::valid_file_hash(digest) || old.digest.as_ref().is_some_and(|previous|previous!=digest) {return Err(FileManagerError::Conflict);}
            old.digest=Some(digest.clone());
        }
        if let Some(context)=&incoming.context {
            if old.context.as_ref().is_some_and(|previous|previous!=context) {return Err(FileManagerError::Conflict);}
            old.context=Some(context.clone());
        }
        if let Some(receipt)=&incoming.receipt {
            receipt.validate_publication_facts().map_err(|_|FileManagerError::Conflict)?;
            if let Some(result)=&receipt.metadata_result {
                result.validate_for(&pab_protocol::FileTransferMetadata {modified_at_unix_ms:spec.modified_at_unix_ms,unix_mode:spec.unix_mode}).map_err(|_|FileManagerError::Conflict)?;
            }
            if receipt.request_id!=attempt || receipt.size>spec.size.0 || receipt.offset>receipt.size || receipt.execution_context.as_ref().is_some_and(|context|old.context.as_ref()!=Some(context)) || receipt.sha256.as_ref().is_some_and(|hash|old.digest.as_ref().is_some_and(|previous|previous!=hash)) {return Err(FileManagerError::Conflict);}
            let mut stale=false;
            if let Some(previous)=&old.receipt {
                // A monitor can capture progress before the publication guard
                // and only acquire the writer after that guard commits. This
                // is delayed evidence from the same attempt, not a reason to
                // cancel a valid data stream or replace a newer receipt.
                stale=!receipt.supersedes(previous).map_err(|_|FileManagerError::Conflict)?;
            }
            if !stale {old.receipt=Some(receipt.clone());}
        }
        if let Some(checkpoint)=incoming.local_checkpoint {
            if checkpoint.0>spec.size.0 {return Err(FileManagerError::Conflict);}
            old.local_checkpoint=Some(old.local_checkpoint.map_or(checkpoint,|previous|previous.max(checkpoint)));
        }
        if let Some(requested)=&incoming.local_metadata_requested {
            requested.validate().map_err(|_|FileManagerError::Conflict)?;
            if requested.modified_at_unix_ms!=spec.modified_at_unix_ms || requested.unix_mode!=spec.unix_mode
                || old.local_metadata_requested.as_ref().is_some_and(|previous|previous!=requested)
                || old.local_publication_intent && old.local_metadata_requested.is_none()
            {return Err(FileManagerError::Conflict);}
            old.local_metadata_requested=Some(requested.clone());
        }
        if incoming.local_publication_intent {
            if old.digest.is_none() || old.context.is_none() || old.local_checkpoint!=Some(spec.size) {return Err(FileManagerError::Conflict);}
            old.local_publication_intent=true;
        }
        if incoming.local_published {
            if !old.local_publication_intent || old.local_checkpoint!=Some(spec.size) || old.digest.is_none() || old.context.is_none() {return Err(FileManagerError::Conflict);}
            old.local_published=true;
        }
        if let Some(result)=&incoming.local_metadata_result {
            if !old.local_published {return Err(FileManagerError::Conflict);}
            result.validate_for(old.local_metadata_requested.as_ref().ok_or(FileManagerError::Conflict)?).map_err(|_|FileManagerError::Conflict)?;
            if old.local_metadata_result.as_ref().is_some_and(|previous|previous!=result) {return Err(FileManagerError::Conflict);}
            old.local_metadata_result=Some(result.clone());
        }
        sqlx::query("INSERT INTO fm_attempt_evidence(attempt,data) VALUES(?,?) ON CONFLICT(attempt) DO UPDATE SET data=excluded.data")
            .bind(attempt.to_string()).bind(serde_json::to_string(&old)?).execute(&mut *tx).await?;
        tx.commit().await?;Ok(())
    }
    pub async fn release_for_observation(&self,owner:&str,item:RequestId,worker:RequestId)->Result<(),FileManagerError> {
        let changed=sqlx::query("UPDATE fm_items SET worker=NULL WHERE id=? AND worker=? AND state IN ('reconciling','unconfirmed') AND EXISTS(SELECT 1 FROM fm_batches b WHERE b.id=fm_items.batch AND b.owner=?)")
            .bind(item.to_string()).bind(worker.to_string()).bind(owner).execute(&self.pool).await?.rows_affected();
        if changed!=1 {return Err(FileManagerError::Conflict);}Ok(())
    }
    pub async fn claim_for_observation(&self,owner:&str,item:RequestId,worker:RequestId)->Result<pab_protocol::FileBatchItem,FileManagerError> {
        let changed=sqlx::query("UPDATE fm_items SET worker=? WHERE id=? AND worker IS NULL AND state IN ('reconciling','unconfirmed') AND EXISTS(SELECT 1 FROM fm_workers w WHERE w.id=? AND w.owner=? AND w.heartbeat>=?) AND EXISTS(SELECT 1 FROM fm_batches b WHERE b.id=fm_items.batch AND b.owner=?)")
            .bind(worker.to_string()).bind(item.to_string()).bind(worker.to_string()).bind(owner).bind(crate::now_ms()-30_000).bind(owner).execute(&self.pool).await?.rows_affected();
        if changed!=1 {return Err(FileManagerError::Conflict);}self.item(owner,item).await
    }
}
