use super::{TaskStore, TaskStoreError};
use pab_protocol::{
    ExecutionContext, FileTransferOperation, FileTransferRequest, OperatorRef, RequestId,
    TransferSnapshot,
};
use sha2::{Digest, Sha256};
use sqlx::Row;

fn fingerprint(request: &FileTransferRequest) -> Result<String, TaskStoreError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(request)?)
    ))
}
impl TaskStore {
    pub async fn existing_transfer(
        &self,
        actor: OperatorRef,
        request: &FileTransferRequest,
    ) -> Result<Option<TransferSnapshot>, TaskStoreError> {
        let row=sqlx::query("SELECT o.initiated_by_json,e.fingerprint FROM transfer_operations o LEFT JOIN transfer_execution e ON e.request_id=o.request_id WHERE o.request_id=?")
            .bind(request.request_id.to_string()).fetch_optional(&self.pool).await?;
        let Some(row) = row else {
            return Ok(None);
        };
        if serde_json::from_str::<OperatorRef>(row.try_get("initiated_by_json")?)? != actor {
            return Err(TaskStoreError::NotFound);
        }
        if row.try_get::<Option<String>, _>("fingerprint")?.as_deref()
            != Some(fingerprint(request)?.as_str())
        {
            return Err(TaskStoreError::RequestConflict);
        }
        self.get_transfer(actor, request.request_id).await.map(Some)
    }
    pub async fn transfer_request(
        &self,
        actor: OperatorRef,
        id: RequestId,
    ) -> Result<FileTransferRequest, TaskStoreError> {
        let row=sqlx::query("SELECT o.initiated_by_json,e.request_json FROM transfer_operations o JOIN transfer_execution e ON e.request_id=o.request_id WHERE o.request_id=?")
            .bind(id.to_string()).fetch_optional(&self.pool).await?.ok_or(TaskStoreError::NotFound)?;
        if serde_json::from_str::<OperatorRef>(row.try_get("initiated_by_json")?)? != actor {
            return Err(TaskStoreError::NotFound);
        }
        Ok(serde_json::from_str(row.try_get("request_json")?)?)
    }
    /// Identity and the accepted operation become durable in one transaction.
    /// Replayed IDs never acquire a new identity or execute a second stream.
    pub async fn accept_transfer(
        &self,
        actor: OperatorRef,
        request: &FileTransferRequest,
        context: &ExecutionContext,
    ) -> Result<bool, TaskStoreError> {
        let fingerprint = fingerprint(request)?;
        let mut tx = self.write_transaction().await?;
        if let Some(row)=sqlx::query("SELECT o.initiated_by_json,e.fingerprint FROM transfer_operations o LEFT JOIN transfer_execution e ON e.request_id=o.request_id WHERE o.request_id=?")
            .bind(request.request_id.to_string()).fetch_optional(&mut *tx).await? {
            if serde_json::from_str::<OperatorRef>(row.try_get("initiated_by_json")?)? != actor { return Err(TaskStoreError::NotFound); }
            if row.try_get::<Option<String>,_>("fingerprint")?.as_deref()!=Some(&fingerprint) { return Err(TaskStoreError::RequestConflict); }
            tx.commit().await?;return Ok(false);
        }
        let used:i64=sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM task_records WHERE request_id=?) + (SELECT COUNT(*) FROM read_operations WHERE request_id=?)")
            .bind(request.request_id.to_string()).bind(request.request_id.to_string()).fetch_one(&mut *tx).await?;
        if used != 0 {
            return Err(TaskStoreError::RequestConflict);
        }
        let identity_matches = match request.execution {
            pab_protocol::ExecutionSelection::Service {} => {
                context.identity.as_ref().is_none_or(|identity| {
                    identity.mode == pab_protocol::ExecutionMode::Service
                        && identity.validate().is_ok()
                })
            }
            pab_protocol::ExecutionSelection::User { .. } => context
                .identity
                .as_ref()
                .is_some_and(|identity| identity.mode == pab_protocol::ExecutionMode::User),
            _ => false,
        };
        if !identity_matches {
            return Err(TaskStoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "transfer execution identity does not match its selection",
            )));
        }
        let (size, hash) = match &request.operation {
            FileTransferOperation::Upload { size, sha256, .. } => (*size, Some(sha256.as_str())),
            FileTransferOperation::Download {
                expected_sha256, ..
            } => (0, expected_sha256.as_deref()),
        };
        sqlx::query("INSERT INTO transfer_operations (request_id,initiated_by_json,direction,path,size,sha256,state,started_at_unix_ms) VALUES (?,?,?,?,?,?,'running',?)")
            .bind(request.request_id.to_string()).bind(serde_json::to_string(&actor)?).bind(request.operation.direction()).bind(request.operation.path()).bind(i64::try_from(size).map_err(|_| TaskStoreError::OffsetTooLarge)?).bind(hash).bind(super::operation::now_unix_ms()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO transfer_execution (request_id,fingerprint,request_json,context_json) VALUES (?,?,?,?)")
            .bind(request.request_id.to_string()).bind(fingerprint).bind(serde_json::to_string(request)?).bind(serde_json::to_string(context)?).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_service::transfer_tests::actor;
    use pab_protocol::*;

    #[tokio::test]
    async fn recovered_terminal_receipt_uses_the_exact_persisted_completion_fact() {
        let root=tempfile::tempdir().unwrap();let database=root.path().join("tasks.sqlite3");let store=TaskStore::open(&database).await.unwrap();let id=RequestId::new();
        // Deterministic clock boundary: emulate the database's authoritative
        // completion timestamp differing from the reader's current clock.
        sqlx::query("CREATE TRIGGER fixture_completion_time AFTER UPDATE OF state ON transfer_operations WHEN NEW.state='completed' BEGIN UPDATE transfer_operations SET finished_at_unix_ms=123456789 WHERE request_id=NEW.request_id; END").execute(&store.pool).await.unwrap();
        store.start_transfer(id,actor(),"receive","/not-opened",4,Some(&"0".repeat(64))).await.unwrap();store.transfer_progress(id,4,4).await.unwrap();store.begin_transfer_publication(id).await.unwrap();store.confirm_transfer_publication(id).await.unwrap();
        let recovered=store.get_transfer(actor(),id).await.unwrap();assert_eq!(recovered.state,"completed");assert_eq!(recovered.published,Some(true));assert_eq!(recovered.finished_at_unix_ms,Some(123456789));
        assert_eq!(store.get_transfer(actor(),id).await.unwrap(),recovered);store.pool.close().await;
        assert_eq!(TaskStore::open(&database).await.unwrap().get_transfer(actor(),id).await.unwrap(),recovered);
    }

    #[tokio::test]
    async fn durable_publication_is_monotonic_but_never_claims_unfinished_attributes() {
        let root=tempfile::tempdir().unwrap();let database=root.path().join("tasks.sqlite3");let store=TaskStore::open(&database).await.unwrap();
        let context=pab_platform::detect_native_execution_context().unwrap();let native=pab_file_manager::encode_path(&root.path().join("must-not-be-reopened.bin"));
        let request=FileTransferRequest { destination_parent:None,source_constraint:None,metadata:Some(FileTransferMetadata {modified_at_unix_ms:Some(1_700_000_000_123),unix_mode:None}),expected_execution:None,native_path:Some(native.clone()),request_id:RequestId::new(),execution:Default::default(),resume_from:None,operation:FileTransferOperation::Upload {path:native.display_label(),size:4,sha256:format!("{:x}",Sha256::digest(b"data")),overwrite:false}};
        store.accept_transfer(actor(),&request,&context).await.unwrap();
        let outcome=FileTransferMetadataResult {requested:request.metadata.clone().unwrap(),actual:request.metadata.clone().unwrap(),warnings:vec![]};
        assert!(store.record_transfer_metadata(request.request_id,&outcome).await.is_err());
        assert!(store.confirm_transfer_publication(request.request_id).await.is_err());
        store.transfer_progress(request.request_id,4,4).await.unwrap();
        assert!(store.confirm_transfer_publication(request.request_id).await.is_err());
        store.begin_transfer_publication(request.request_id).await.unwrap();
        assert_eq!(store.get_transfer(actor(),request.request_id).await.unwrap().published,None);
        store.confirm_transfer_publication(request.request_id).await.unwrap();
        let committed:i64=sqlx::query_scalar("SELECT committed_at_unix_ms FROM transfer_publications WHERE request_id=?").bind(request.request_id.to_string()).fetch_one(&store.pool).await.unwrap();
        store.publication_failed(request.request_id,"late worker error").await.unwrap();store.finish_transfer(request.request_id,"failed",Some("late stream error")).await.unwrap();
        store.confirm_transfer_publication(request.request_id).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT committed_at_unix_ms FROM transfer_publications WHERE request_id=?").bind(request.request_id.to_string()).fetch_one(&store.pool).await.unwrap(),committed);
        let pending=store.request_transfer_stop(actor(),request.request_id,FileTransferIntent::Cancel).await.unwrap();
        assert_eq!(pending.state,"committing");assert_eq!(pending.published,Some(true));assert_eq!(pending.finished_at_unix_ms,None);assert_eq!(pending.message,None);
        store.finish_transfer(request.request_id,"completed",None).await.unwrap();
        assert_eq!(store.get_transfer(actor(),request.request_id).await.unwrap(),pending);
        store.pool.close().await;
        let reopened=TaskStore::open(&database).await.unwrap();
        assert_eq!(reopened.get_transfer(actor(),request.request_id).await.unwrap(),pending);
        let foreign=OperatorRef::account(UserId::from_u128(555),EndpointKey::new([5;32]));assert!(matches!(reopened.get_transfer(foreign,request.request_id).await,Err(TaskStoreError::NotFound)));
        let mut wrong=outcome.clone();wrong.requested.modified_at_unix_ms=None;
        assert!(reopened.record_transfer_metadata(request.request_id,&wrong).await.is_err());
        reopened.record_transfer_metadata(request.request_id,&outcome).await.unwrap();
        reopened.record_transfer_metadata(request.request_id,&outcome).await.unwrap();
        let completed=reopened.get_transfer(actor(),request.request_id).await.unwrap();assert_eq!(completed.state,"completed");assert_eq!(completed.published,Some(true));assert_eq!(completed.metadata_result,Some(outcome.clone()));
        let mut changed=outcome;changed.actual.modified_at_unix_ms=None;changed.warnings.push(FileTransferMetadataWarning {attribute:FileTransferAttribute::ModifiedTime,code:"unconfirmed".into(),message:"Different later outcome".into()});
        assert!(reopened.record_transfer_metadata(request.request_id,&changed).await.is_err());
        // This internal fact survives losing access to the final pathname. It
        // does not reapply attributes or invent a completed result after restart.
        assert!(!root.path().join("must-not-be-reopened.bin").exists());
    }

    #[tokio::test]
    async fn publication_records_reject_unfinished_send_stopped_and_invalid_hash_attempts() {
        let root=tempfile::tempdir().unwrap();let store=TaskStore::open(&root.path().join("tasks.sqlite3")).await.unwrap();
        for (direction,hash) in [("send",Some("0".repeat(64))),("receive",None),("receive",Some("G".repeat(64)))] {
            let id=RequestId::new();store.start_transfer(id,actor(),direction,"/not-opened",4,hash.as_deref()).await.unwrap();store.transfer_progress(id,4,4).await.unwrap();store.begin_transfer_publication(id).await.unwrap();
            assert!(matches!(store.confirm_transfer_publication(id).await,Err(TaskStoreError::RequestConflict)));
        }
        for state in ["running","failed","interrupted","paused","cancelled","stopping"] {
            let id=RequestId::new();store.start_transfer(id,actor(),"receive","/not-opened",4,Some(&"0".repeat(64))).await.unwrap();store.transfer_progress(id,4,4).await.unwrap();
            sqlx::query("UPDATE transfer_operations SET state=? WHERE request_id=?").bind(state).bind(id.to_string()).execute(&store.pool).await.unwrap();
            assert!(matches!(store.confirm_transfer_publication(id).await,Err(TaskStoreError::RequestConflict)),"{state}");
        }
        let id=RequestId::new();store.start_transfer(id,actor(),"receive","/not-opened",4,Some(&"0".repeat(64))).await.unwrap();store.transfer_progress(id,4,4).await.unwrap();store.begin_transfer_publication(id).await.unwrap();store.confirm_transfer_publication(id).await.unwrap();
        let snapshot=store.get_transfer(actor(),id).await.unwrap();assert_eq!(snapshot.state,"completed");assert_eq!(snapshot.published,Some(true));
        store.confirm_transfer_publication(id).await.unwrap();
    }

    #[tokio::test]
    async fn transfer_acceptance_is_atomic_deduplicated_owned_and_keeps_identity_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.sqlite3");
        let store = TaskStore::open(&path).await.unwrap();
        let mut context = pab_platform::detect_native_execution_context().unwrap();
        context.identity = Some(
            pab_os_control::execution::current_identity()
                .unwrap()
                .observation(
                    ExecutionMode::User,
                    ExecutionEnvironmentSource::NativeAccount,
                )
                .unwrap(),
        );
        let target = dir.path().join("existing.bin");
        std::fs::write(&target, b"original").unwrap();
        let request = FileTransferRequest { destination_parent:None, source_constraint:None,metadata:None,expected_execution:None,
            native_path: None,
            request_id: RequestId::new(),
            execution: ExecutionSelection::User {
                context_ref: ExecutionContextRef::new(),
            },
            resume_from: None,
            operation: FileTransferOperation::Upload {
                path: target.to_str().unwrap().into(),
                size: 8,
                sha256: format!("{:x}", Sha256::digest(b"original")),
                overwrite: true,
            },
        };
        let mut missing = context.clone();
        missing.identity = None;
        assert!(
            store
                .accept_transfer(actor(), &request, &missing)
                .await
                .is_err()
        );
        assert!(matches!(
            store.get_transfer(actor(), request.request_id).await,
            Err(TaskStoreError::NotFound)
        ));
        let (a, b) = tokio::join!(
            store.accept_transfer(actor(), &request, &context),
            store.accept_transfer(actor(), &request, &context)
        );
        assert_ne!(a.unwrap(), b.unwrap());
        let mut different = context.clone();
        different.cwd = Some("must-not-replace-original".into());
        assert!(
            !store
                .accept_transfer(actor(), &request, &different)
                .await
                .unwrap()
        );
        let mut changed = request.clone();
        changed.execution = Default::default();
        assert!(matches!(
            store.existing_transfer(actor(), &changed).await,
            Err(TaskStoreError::RequestConflict)
        ));
        let foreign = OperatorRef::account(UserId::from_u128(555), EndpointKey::new([5; 32]));
        assert!(matches!(
            store.existing_transfer(foreign, &request).await,
            Err(TaskStoreError::NotFound)
        ));
        store
            .transfer_progress(request.request_id, 8, 8)
            .await
            .unwrap();
        store
            .begin_transfer_publication(request.request_id)
            .await
            .unwrap();
        drop(store);
        let reopened = TaskStore::open(&path).await.unwrap();
        let prior = reopened
            .existing_transfer(actor(), &request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(prior.execution_context, Some(context));
        // Even though the service can read a matching file, only TaskService's
        // original-user worker may reconcile a user transfer.
        assert_eq!(prior.state, "committing");
        assert_eq!(prior.published, None);
        assert_eq!(
            reopened
                .transfer_request(actor(), request.request_id)
                .await
                .unwrap(),
            request
        );
    }
}
