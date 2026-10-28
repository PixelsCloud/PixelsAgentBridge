//! Receipt retention spans the local mutation and batch database transactions.
//! A crash between those commits must preserve facts, never infer an ACK from
//! elapsed time or a batch's terminal state.
use super::{Body,LocalFileBrowser,LocalFileManager,FileManagerError,Op};
use pab_protocol::{FileManagerReply,FileManagerRequest,ExecutionSelection,RequestId,FILE_MANAGER_SCHEMA_VERSION};
use sqlx::{Row,SqlitePool};

#[derive(Debug,Clone,PartialEq,Eq)]
pub struct FileMutationReceipt {pub request_id:RequestId,pub operation:Op,pub result:Body}
impl FileMutationReceipt {
    pub(crate) fn validate(&self)->Result<(),FileManagerError> {
        let request=FileManagerRequest {schema_version:FILE_MANAGER_SCHEMA_VERSION,request_id:self.request_id,execution:ExecutionSelection::Service {},expected_execution:None,operation:self.operation.clone()};
        request.validate().map_err(FileManagerError::Invalid)?;
        FileManagerReply {schema_version:FILE_MANAGER_SCHEMA_VERSION,request_id:self.request_id,execution_context:None,result:self.result.clone()}.validate_for(&request).map_err(|_|FileManagerError::Conflict)?;
        if matches!(&self.result,Body::Error {error} if matches!(error.code.as_str(),"unconfirmed"|"outcome_unconfirmed")) {return Err(FileManagerError::Interrupted);}
        Ok(())
    }
}

pub(super) async fn initialize(journal:&SqlitePool)->Result<(),FileManagerError> {
    let mut tx=crate::begin_write_transaction(journal,"UPDATE local_manager_requests SET created=created WHERE 0").await?;
    let columns=sqlx::query("PRAGMA table_info(local_manager_requests)").fetch_all(&mut *tx).await?;
    if !columns.iter().any(|row|row.try_get::<&str,_>("name").ok()==Some("retained")) {
        sqlx::query("ALTER TABLE local_manager_requests ADD COLUMN retained INTEGER NOT NULL DEFAULT 0").execute(&mut *tx).await?;
        // Old batches may already be waiting for an original confirmed receipt.
        // This migration is in the same transaction as the column addition.
        sqlx::query("UPDATE local_manager_requests SET retained=1 WHERE owner GLOB 'batch:*'").execute(&mut *tx).await?;
    }
    sqlx::query("CREATE INDEX IF NOT EXISTS local_manager_retained ON local_manager_requests(owner,id) WHERE retained=1 AND reply IS NOT NULL").execute(&mut *tx).await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS local_manager_expiry ON local_manager_requests(created) WHERE retained=0 AND reply IS NOT NULL").execute(&mut *tx).await?;
    tx.commit().await?;Ok(())
}

impl LocalFileBrowser {
    /// A bounded stable-ID page, scoped to this browser's authenticated owner.
    /// Pending operations without a result remain retained but cannot be ACKed.
    pub async fn retained_receipts(&self,after:Option<RequestId>)->Result<Vec<FileMutationReceipt>,FileManagerError> {
        let rows=sqlx::query("SELECT id,spec,reply FROM local_manager_requests WHERE owner=? AND retained=1 AND reply IS NOT NULL AND id>? ORDER BY id LIMIT 128")
            .bind(&self.owner).bind(after.map(|id|id.to_string()).unwrap_or_default()).fetch_all(&self.manager.journal).await?;
        rows.iter().map(decode_receipt).collect()
    }
    /// Release only the exact original fact that the coordinator persisted.
    /// Missing/already released rows are a no-op; another owner's row, a wrong
    /// operation/result or an unconfirmed outcome never releases a retention.
    pub async fn acknowledge_receipt(&self,expected:&FileMutationReceipt)->Result<bool,FileManagerError> {
        expected.validate()?;
        let mut tx=crate::begin_write_transaction(&self.manager.journal,"UPDATE local_manager_requests SET created=created WHERE 0").await?;
        let row=sqlx::query("SELECT owner,spec,reply FROM local_manager_requests WHERE id=?").bind(expected.request_id.to_string()).fetch_optional(&mut *tx).await?;
        let Some(row)=row else {return Ok(false)};
        if row.try_get::<&str,_>("owner")?!=self.owner {return Err(FileManagerError::NotFound);}
        if row.try_get::<&str,_>("spec")?!=serde_json::to_string(&expected.operation)? {return Err(FileManagerError::Conflict);}
        let data=row.try_get::<Option<&str>,_>("reply")?.ok_or(FileManagerError::Interrupted)?;
        let actual:FileManagerReply=serde_json::from_str(data)?;
        if actual.schema_version!=FILE_MANAGER_SCHEMA_VERSION || actual.request_id!=expected.request_id || actual.result!=expected.result {return Err(FileManagerError::Conflict);}
        let changed=sqlx::query("UPDATE local_manager_requests SET retained=0 WHERE id=? AND owner=? AND retained=1").bind(expected.request_id.to_string()).bind(&self.owner).execute(&mut *tx).await?.rows_affected();
        tx.commit().await?;Ok(changed!=0)
    }
    /// Repair a lost local ACK after a crash. Only a matching result already
    /// present in the owning batch database permits release; unknowns stay held.
    pub async fn release_recorded_receipts(&self,store:&crate::BatchStore,owner:&str,batch:RequestId)->Result<u64,FileManagerError> {
        if self.owner!=format!("batch:{batch}") {return Err(FileManagerError::NotFound);}
        let valid:Option<i64>=sqlx::query_scalar("SELECT 1 FROM fm_batches WHERE id=? AND owner=?").bind(batch.to_string()).bind(owner).fetch_optional(&store.pool).await?;
        if valid.is_none() {return Err(FileManagerError::NotFound);}
        let mut after=None;let mut released=0;
        loop {
            let receipts=self.retained_receipts(after).await?;
            let Some(last)=receipts.last() else {break};after=Some(last.request_id);
            for original in receipts {
                if let Some(recorded)=store.recorded_directory_receipt(owner,batch,original.request_id).await? {
                    if original!=recorded {return Err(FileManagerError::Conflict);}
                    released+=u64::from(self.acknowledge_receipt(&recorded).await?);
                }
            }
        }
        Ok(released)
    }
}

fn decode_receipt(row:&sqlx::sqlite::SqliteRow)->Result<FileMutationReceipt,FileManagerError> {
    let request_id=row.try_get::<&str,_>("id")?.parse().map_err(|_|FileManagerError::Conflict)?;
    let reply:FileManagerReply=serde_json::from_str(row.try_get("reply")?)?;
    if reply.schema_version!=FILE_MANAGER_SCHEMA_VERSION || reply.request_id!=request_id {return Err(FileManagerError::Conflict);}
    Ok(FileMutationReceipt {request_id,operation:serde_json::from_str(row.try_get("spec")?)?,result:reply.result})
}

impl LocalFileManager {
    /// Window startup also handles terminal batches that will never be prepared
    /// again. Other accounts' receipts and missing/unknown batch facts stay held.
    /// This is an explicit maintenance pass, not a per-poll background sweep.
    pub async fn release_recorded_batch_receipts(&self,store:&crate::BatchStore,account_owner:&str)->Result<u64,FileManagerError> {
        let (mut after_owner,mut after_id)=(String::new(),String::new());let mut released=0;
        loop {
            let rows=sqlx::query("SELECT owner,id,spec,reply FROM local_manager_requests WHERE retained=1 AND reply IS NOT NULL AND owner GLOB 'batch:*' AND (owner>? OR (owner=? AND id>?)) ORDER BY owner,id LIMIT 128")
                .bind(&after_owner).bind(&after_owner).bind(&after_id).fetch_all(&self.journal).await?;
            let Some(last)=rows.last() else {break};after_owner=last.try_get("owner")?;after_id=last.try_get("id")?;
            for row in rows {
                let label=row.try_get::<&str,_>("owner")?;
                let Some(batch)=label.strip_prefix("batch:").and_then(|id|id.parse::<RequestId>().ok()).filter(|id|format!("batch:{id}")==label) else {continue};
                let valid:Option<i64>=sqlx::query_scalar("SELECT 1 FROM fm_batches WHERE id=? AND owner=?").bind(batch.to_string()).bind(account_owner).fetch_optional(&store.pool).await?;
                if valid.is_none() {continue;}
                let original=decode_receipt(&row)?;
                if let Some(recorded)=store.recorded_directory_receipt(account_owner,batch,original.request_id).await? {
                    if original!=recorded {return Err(FileManagerError::Conflict);}
                    released+=u64::from(self.browser(label.into()).acknowledge_receipt(&recorded).await?);
                }
            }
        }
        Ok(released)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pab_protocol::*;
    use std::path::Path;
    use tokio::sync::watch;

    fn mkdir(root:&Path,name:&str)->Op {Op::CreateDirectory {parent:crate::encode_path(root),name:NativeFilePath::Utf8(name.into())}}
    async fn gc(manager:&LocalFileManager,root:&Path,name:&str) {
        let (_send,cancel)=watch::channel(false);
        assert!(matches!(manager.execute("ui",RequestId::new(),&mkdir(root,name),cancel).await.unwrap().result,Body::Applied {..}));
    }
    async fn close(manager:&LocalFileManager) {manager.index.clone().shutdown().await;manager.journal.close().await;}
    async fn age(manager:&LocalFileManager,id:RequestId) {
        sqlx::query("UPDATE local_manager_requests SET created=? WHERE id=?").bind(crate::now_ms()-172_800_000).bind(id.to_string()).execute(&manager.journal).await.unwrap();
    }
    #[tokio::test]
    async fn acknowledgement_checks_owner_operation_and_result_and_only_then_allows_ttl_collection() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let manager=LocalFileManager::open(&root.join("local.db")).await.unwrap();let browser=manager.browser("coordinator".into());
        let (_send,cancel)=watch::channel(false);let id=RequestId::new();let operation=mkdir(&root,"held");
        let original=browser.execute_request(id,operation.clone(),cancel.clone()).await.unwrap();let fact=FileMutationReceipt {request_id:id,operation:operation.clone(),result:original.result.clone()};
        age(&manager,id).await;let ui_id=RequestId::new();manager.execute("ui",ui_id,&mkdir(&root,"ordinary"),cancel.clone()).await.unwrap();age(&manager,ui_id).await;
        let unknown=RequestId::new();let unknown_op=mkdir(&root,"unknown");
        sqlx::query("INSERT INTO local_manager_requests(id,owner,spec,created,retained) VALUES(?,?,?,?,1)").bind(unknown.to_string()).bind("coordinator").bind(serde_json::to_string(&unknown_op).unwrap()).bind(crate::now_ms()-172_800_000).execute(&manager.journal).await.unwrap();
        gc(&manager,&root,"first-collection").await;
        assert!(matches!(manager.observe("ui",ui_id,&mkdir(&root,"ordinary")).await,Err(FileManagerError::NotFound)));
        assert_eq!(browser.observe_request(id,&operation).await.unwrap(),Some(original.clone()));
        assert!(matches!(manager.browser("foreign".into()).acknowledge_receipt(&fact).await,Err(FileManagerError::NotFound)));
        let mut wrong=fact.clone();wrong.operation=mkdir(&root,"other");assert!(browser.acknowledge_receipt(&wrong).await.is_err());
        wrong=fact.clone();wrong.result=Body::Error {error:FileSystemError {code:"filesystem_io".into(),phase:"local_files".into(),message:"different fact".into()}};assert!(matches!(browser.acknowledge_receipt(&wrong).await,Err(FileManagerError::Conflict)));
        wrong.result=Body::Error {error:FileSystemError {code:"unconfirmed".into(),phase:"reconcile".into(),message:"unknown".into()}};assert!(matches!(browser.acknowledge_receipt(&wrong).await,Err(FileManagerError::Interrupted)));
        let pending=FileMutationReceipt {request_id:unknown,operation:unknown_op.clone(),result:Body::Applied {path:crate::encode_path(&root.join("unknown"))}};
        assert!(matches!(browser.acknowledge_receipt(&pending).await,Err(FileManagerError::Interrupted)));
        assert_eq!(browser.retained_receipts(None).await.unwrap(),vec![fact.clone()]);
        assert!(browser.acknowledge_receipt(&fact).await.unwrap());assert!(!browser.acknowledge_receipt(&fact).await.unwrap());
        gc(&manager,&root,"second-collection").await;
        assert!(matches!(browser.observe_request(id,&operation).await,Err(FileManagerError::NotFound)));assert!(!browser.acknowledge_receipt(&fact).await.unwrap());
        assert!(browser.observe_request(unknown,&unknown_op).await.unwrap().is_none());assert!(!root.join("unknown").exists());
        close(&manager).await;temp.close().unwrap();assert!(!root.exists());
    }
    #[tokio::test]
    async fn migration_pins_legacy_batch_facts_once_and_preserves_unknown_records() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let db=root.join("legacy.db");
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&db).create_if_missing(true)).await.unwrap();
        sqlx::query("CREATE TABLE local_manager_requests(id TEXT PRIMARY KEY,owner TEXT NOT NULL,spec TEXT NOT NULL,reply TEXT,created INTEGER NOT NULL)").execute(&pool).await.unwrap();
        let batch_owner=format!("batch:{}",RequestId::new());let operation=mkdir(&root,"legacy");let id=RequestId::new();let unknown=RequestId::new();let ui_id=RequestId::new();
        let frame=FileManagerReply {schema_version:FILE_MANAGER_SCHEMA_VERSION,request_id:id,execution_context:None,result:Body::Applied {path:crate::encode_path(&root.join("legacy"))}};
        for (key,owner,reply) in [(id,batch_owner.as_str(),Some(serde_json::to_string(&frame).unwrap())),(unknown,"ui-unknown",None),(ui_id,"ui",Some(serde_json::to_string(&FileManagerReply {request_id:ui_id,..frame.clone()}).unwrap()))] {
            sqlx::query("INSERT INTO local_manager_requests(id,owner,spec,reply,created) VALUES(?,?,?,?,?)").bind(key.to_string()).bind(owner).bind(serde_json::to_string(&operation).unwrap()).bind(reply).bind(crate::now_ms()-172_800_000).execute(&pool).await.unwrap();
        }
        let manager=LocalFileManager::open(&db).await.unwrap();gc(&manager,&root,"gc-after-migration").await;
        let browser=manager.browser(batch_owner.clone());assert_eq!(browser.observe_request(id,&operation).await.unwrap(),Some(frame.clone()));assert!(manager.observe("ui-unknown",unknown,&operation).await.unwrap().is_none());
        assert!(matches!(manager.observe("ui",ui_id,&operation).await,Err(FileManagerError::NotFound)));assert!(!root.join("legacy").exists(),"migration must not replay old operations");
        let fact=FileMutationReceipt {request_id:id,operation:operation.clone(),result:frame.result};assert!(browser.acknowledge_receipt(&fact).await.unwrap());
        let reopened=LocalFileManager::open(&db).await.unwrap();assert!(reopened.browser(batch_owner).retained_receipts(None).await.unwrap().is_empty(),"reopening must not repin acknowledged legacy facts");
        let held:i64=sqlx::query_scalar("SELECT retained FROM local_manager_requests WHERE id=?").bind(id.to_string()).fetch_one(&pool).await.unwrap();assert_eq!(held,0);
        pool.close().await;close(&manager).await;close(&reopened).await;temp.close().unwrap();assert!(!root.exists());
    }
    #[tokio::test]
    async fn retained_receipt_pages_are_bounded_owner_scoped_and_have_no_gaps() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let manager=LocalFileManager::open(&root.join("pages.db")).await.unwrap();
        let operation=mkdir(&root,"storage-fixture");let mut ids=Vec::new();let mut tx=manager.journal.begin().await.unwrap();
        for index in 0..131 {
            let id=RequestId::new();let owner=if index==130 {"foreign"}else{"owner"};
            let reply=FileManagerReply {schema_version:FILE_MANAGER_SCHEMA_VERSION,request_id:id,execution_context:None,result:Body::Applied {path:crate::encode_path(&root.join("storage-fixture"))}};
            sqlx::query("INSERT INTO local_manager_requests(id,owner,spec,reply,created,retained) VALUES(?,?,?,?,?,1)").bind(id.to_string()).bind(owner).bind(serde_json::to_string(&operation).unwrap()).bind(serde_json::to_string(&reply).unwrap()).bind(crate::now_ms()).execute(&mut *tx).await.unwrap();
            if owner=="owner" {ids.push(id);}
        }
        tx.commit().await.unwrap();ids.sort_by_key(|id|id.to_string());let browser=manager.browser("owner".into());
        let first=browser.retained_receipts(None).await.unwrap();assert_eq!(first.len(),128);let second=browser.retained_receipts(Some(first.last().unwrap().request_id)).await.unwrap();assert_eq!(second.len(),2);
        assert!(browser.retained_receipts(Some(second.last().unwrap().request_id)).await.unwrap().is_empty());assert_eq!(first.iter().chain(&second).map(|r|r.request_id).collect::<Vec<_>>(),ids);
        assert_eq!(manager.browser("foreign".into()).retained_receipts(None).await.unwrap().len(),1);assert!(!root.join("storage-fixture").exists(),"storage fixtures never execute a mutation");
        close(&manager).await;temp.close().unwrap();assert!(!root.exists());
    }
    #[tokio::test]
    async fn restart_releases_only_results_recorded_in_the_owning_batch_even_when_terminal() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let local_db=root.join("local.db");let batch_db=root.join("batch.db");
        let manager=LocalFileManager::open(&local_db).await.unwrap();let store=crate::BatchStore::open(&batch_db).await.unwrap();let destination=root.join("destination");let source=root.join("source");std::fs::create_dir(&destination).unwrap();std::fs::create_dir(&source).unwrap();
        let native=crate::encode_path(&destination);let identity=crate::DirectoryObject::open(&native,false).unwrap().identity().to_owned();
        let spec=FileBatchSpec {id:RequestId::new(),device:DeviceRef {tenant_id:TenantId::from_u128(1),device_id:DeviceId::from_u128(2)},code:"123456789".parse().unwrap(),direction:FileTransferDirection::Download,sources:vec![crate::encode_path(&source)],destination:native.clone(),execution:ExecutionSelection::Service {},conflict:FileConflictPolicy::Ask,force_relay:false};
        store.create_with_destination("account",&spec,&crate::DirectoryReady {path:native,identity}).await.unwrap();
        let entry=FileBatchItemSpec {id:RequestId::new(),source:crate::encode_path(&source),destination:crate::encode_path(&destination.join("folder")),relative_name:"folder".into(),kind:ManagedFileKind::Directory,size:FileByteCount(0),modified_at_unix_ms:None,unix_mode:None};
        store.append("account",spec.id,&[entry.clone()]).await.unwrap();store.finish_scan("account",spec.id,None).await.unwrap();let worker=RequestId::new();store.heartbeat("account",worker).await.unwrap();store.claim("account",spec.id,worker).await.unwrap().unwrap();
        store.save_item_target("account",entry.id,worker,&crate::ItemTarget {path:entry.destination.clone(),overwrite:false,choice:FileConflictPolicy::Ask}).await.unwrap();
        let style=if cfg!(windows){PathStyle::Windows}else{PathStyle::Posix};let preparation=store.begin_directory_preparation("account",entry.id,worker,&entry.destination,style,None).await.unwrap().unwrap();
        let label=format!("batch:{}",spec.id);let browser=manager.browser(label.clone());let (_send,cancel)=watch::channel(false);
        let prepared=browser.execute_request(preparation.request_id,preparation.operation.clone(),cancel.clone()).await.unwrap();age(&manager,preparation.request_id).await;gc(&manager,&root,"gc-unknown").await;
        assert_eq!(browser.release_recorded_receipts(&store,"account",spec.id).await.unwrap(),0);assert_eq!(browser.retained_receipts(None).await.unwrap().len(),1);
        store.record_directory_preparation("account",entry.id,worker,preparation.request_id,&prepared.result).await.unwrap();
        // Simulate coordinator exit after its fact commit but before the local ACK.
        let reopened=LocalFileManager::open(&local_db).await.unwrap();let batches=crate::BatchStore::open(&batch_db).await.unwrap();
        assert_eq!(reopened.release_recorded_batch_receipts(&batches,"foreign-account").await.unwrap(),0);
        let mut altered=prepared.clone();let Body::DirectoryPrepared {identity,..}=&mut altered.result else {panic!("missing prepared fact")};identity.push_str("-different");
        sqlx::query("UPDATE local_manager_requests SET reply=? WHERE id=?").bind(serde_json::to_string(&altered).unwrap()).bind(preparation.request_id.to_string()).execute(&manager.journal).await.unwrap();
        assert!(matches!(reopened.release_recorded_batch_receipts(&batches,"account").await,Err(FileManagerError::Conflict)));assert_eq!(browser.retained_receipts(None).await.unwrap().len(),1);
        sqlx::query("UPDATE local_manager_requests SET reply=? WHERE id=?").bind(serde_json::to_string(&prepared).unwrap()).bind(preparation.request_id.to_string()).execute(&manager.journal).await.unwrap();
        assert_eq!(reopened.release_recorded_batch_receipts(&batches,"account").await.unwrap(),1);assert_eq!(reopened.release_recorded_batch_receipts(&batches,"account").await.unwrap(),0);
        let current=store.claim("account",spec.id,worker).await.unwrap().unwrap();let finalization=store.begin_directory_finalization("account",entry.id,worker).await.unwrap().unwrap();assert_eq!(current.attempt,Some(finalization.request_id));
        let finalized=browser.execute_request(finalization.request_id,finalization.operation.clone(),cancel.clone()).await.unwrap();assert_eq!(browser.release_recorded_receipts(&store,"account",spec.id).await.unwrap(),0);
        store.record_directory_outcome("account",entry.id,worker,finalization.request_id,&finalized.result).await.unwrap();let current=store.item("account",entry.id).await.unwrap();
        store.advance("account",entry.id,worker,current.revision,FileItemState::Completed,FileByteCount(0),None).await.unwrap();assert_eq!(store.snapshot("account",spec.id).await.unwrap().phase,"completed");
        let unrecorded=RequestId::new();let unrelated=mkdir(&root,"unrecorded-original");browser.execute_request(unrecorded,unrelated.clone(),cancel).await.unwrap();age(&manager,unrecorded).await;
        assert_eq!(reopened.release_recorded_batch_receipts(&batches,"account").await.unwrap(),1);assert_eq!(browser.retained_receipts(None).await.unwrap().len(),1,"terminal state must not release an unrecorded original");
        assert!(store.recorded_directory_receipt("foreign-account",spec.id,preparation.request_id).await.is_err());assert!(store.recorded_directory_receipt("account",spec.id,unrecorded).await.unwrap().is_none());
        gc(&reopened,&root,"gc-recorded").await;assert!(browser.observe_request(unrecorded,&unrelated).await.unwrap().is_some());assert!(destination.join("folder").is_dir());
        store.pool.close().await;batches.pool.close().await;close(&manager).await;close(&reopened).await;temp.close().unwrap();assert!(!root.exists());
    }
}
