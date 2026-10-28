//! Current-process identity adapter for the local pane. Durable request IDs
//! prevent a lost WebView response from repeating a filesystem mutation.
use crate::{DirectoryIndex, FileManagerError};
use pab_protocol::{FileManagerOperation as Op, FileManagerReply, FileManagerResult as Body, RequestId};
use sqlx::{Row, SqlitePool, sqlite::SqliteConnectOptions};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::sync::{Mutex, watch};
#[path="local_receipts.rs"]
mod receipts;
pub use receipts::FileMutationReceipt;

#[derive(Clone)]
pub struct LocalFileManager {
    pub index: DirectoryIndex,
    journal: SqlitePool,
    operations: Arc<Mutex<()>>,
    #[cfg(unix)]
    staging_database:std::path::PathBuf,
}
#[derive(Clone)]
pub struct LocalFileBrowser {manager:LocalFileManager,owner:String}
impl LocalFileBrowser {
    /// Explicit coordinator requests retain mutation receipts until the caller
    /// has recorded the same original result and acknowledges it.
    pub async fn execute_request(&self,id:RequestId,operation:Op,cancel:watch::Receiver<bool>)->Result<FileManagerReply,FileManagerError> {self.manager.execute_inner(&self.owner,id,&operation,cancel,true).await}
    pub async fn observe_request(&self,id:RequestId,operation:&Op)->Result<Option<FileManagerReply>,FileManagerError> {self.manager.observe(&self.owner,id,operation).await}
}
impl crate::FileBrowser for LocalFileBrowser {
    fn execute<'a>(&'a self,operation:Op,cancel:watch::Receiver<bool>)->std::pin::Pin<Box<dyn std::future::Future<Output=Result<FileManagerReply,FileManagerError>>+Send+'a>> {
        Box::pin(self.execute_request(RequestId::new(),operation,cancel))
    }
}
impl LocalFileManager {
    #[cfg(test)]
    pub(crate) async fn close_for_test(&self){self.journal.close().await;self.index.clone().shutdown().await;}
    pub fn browser(&self,owner:String)->LocalFileBrowser {LocalFileBrowser {manager:self.clone(),owner}}
    /// Read only the original journal entry. Observation never performs the
    /// mutation again, including after a restart or a lost final reply.
    pub async fn observe(&self,owner:&str,id:RequestId,operation:&Op)->Result<Option<FileManagerReply>,FileManagerError> {
        let row=sqlx::query("SELECT spec,reply FROM local_manager_requests WHERE id=? AND owner=?").bind(id.to_string()).bind(owner).fetch_optional(&self.journal).await?;
        let Some(row)=row else {return Err(FileManagerError::NotFound);};
        if row.try_get::<String,_>("spec")?!=serde_json::to_string(operation)? {return Err(FileManagerError::Conflict);}
        let original:Option<FileManagerReply>=row.try_get::<Option<String>,_>("reply")?.map(|data|serde_json::from_str(&data)).transpose()?;
        #[cfg(unix)]
        if matches!(operation,Op::PrepareDirectory {existing_object:None,..}) && original.as_ref().is_none_or(|reply|matches!(&reply.result,Body::Error {error} if matches!(error.code.as_str(),"unconfirmed"|"outcome_unconfirmed"))) {
            self.reconcile_directory_creations().await?;
            let data:Option<String>=sqlx::query_scalar("SELECT reply FROM local_manager_requests WHERE id=? AND owner=? AND spec=?").bind(id.to_string()).bind(owner).bind(serde_json::to_string(operation)?).fetch_one(&self.journal).await?;
            return data.map(|data|serde_json::from_str(&data)).transpose().map_err(Into::into);
        }
        Ok(original)
    }
    #[cfg(unix)]
    async fn reconcile_directory_creations(&self)->Result<(),FileManagerError> {
        let mut after=None;
        loop {
        let database=self.staging_database.clone();
        let receipts=blocking(move ||crate::directory_creation_receipts(&database,after)).await?;
        if receipts.is_empty(){break;}
        after=receipts.last().map(|receipt|receipt.fact.request_id);
        for receipt in receipts {
            let fact=&receipt.fact;
            let mut tx=crate::begin_write_transaction(&self.journal,"UPDATE local_manager_requests SET created=created WHERE 0").await?;
            let row=sqlx::query("SELECT spec,reply FROM local_manager_requests WHERE id=?").bind(fact.request_id.to_string()).fetch_optional(&mut *tx).await?;
            let Some(row)=row else {continue;};
            let operation:Op=serde_json::from_str(row.try_get("spec")?)?;
            if operation!=fact.operation {return Err(FileManagerError::Conflict);}
            let original:Option<FileManagerReply>=row.try_get::<Option<String>,_>("reply")?.map(|data|serde_json::from_str(&data)).transpose()?;
            if let Some(original)=original {
                if original.request_id!=fact.request_id || original.schema_version!=pab_protocol::FILE_MANAGER_SCHEMA_VERSION {return Err(FileManagerError::Conflict);}
                if original.result!=fact.result && !matches!(original.result,Body::Error {error} if matches!(error.code.as_str(),"unconfirmed"|"outcome_unconfirmed")) {return Err(FileManagerError::Conflict);}
            }
            sqlx::query("UPDATE local_manager_requests SET reply=? WHERE id=?").bind(serde_json::to_string(&reply(fact.request_id,fact.result.clone()))?).bind(fact.request_id.to_string()).execute(&mut *tx).await?;
            tx.commit().await?;
            if let Err(error)=receipt.acknowledge(){eprintln!("file-manager: recovered directory result saved; creation receipt remains for later acknowledgement: {error}");}
        }
        }
        Ok(())
    }
    pub async fn open(database: &Path) -> Result<Self,FileManagerError> {
        #[cfg(unix)]
        {
            let path=database.to_owned();
            let report=blocking(move ||crate::recover_directory_staging(&path)).await?;
            if report.retained>0 {eprintln!("file-manager: {} inactive directory staging receipts retained for diagnosis",report.retained);}
        }
        let index = DirectoryIndex::open(database).await?;
        let journal = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(SqliteConnectOptions::new().filename(database).create_if_missing(true).busy_timeout(Duration::from_secs(10))).await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS local_manager_requests(id TEXT PRIMARY KEY,owner TEXT NOT NULL,spec TEXT NOT NULL,reply TEXT,created INTEGER NOT NULL,retained INTEGER NOT NULL DEFAULT 0)").execute(&journal).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS local_manager_owner ON local_manager_requests(owner,created)").execute(&journal).await?;
        receipts::initialize(&journal).await?;
        let manager=Self {index,journal,operations: Arc::new(Mutex::new(())),#[cfg(unix)] staging_database:database.to_owned()};
        #[cfg(unix)]
        manager.reconcile_directory_creations().await?;
        Ok(manager)
    }
    pub async fn execute(&self, owner: &str, id: RequestId, operation: &Op, cancel: watch::Receiver<bool>) -> Result<FileManagerReply,FileManagerError> {
        self.execute_inner(owner,id,operation,cancel,false).await
    }
    async fn execute_inner(&self, owner: &str, id: RequestId, operation: &Op, cancel: watch::Receiver<bool>, retain_receipt:bool) -> Result<FileManagerReply,FileManagerError> {
        if owner.is_empty() || owner.len() > 1024 { return Err(FileManagerError::Invalid("invalid local file session")); }
        let request = pab_protocol::FileManagerRequest {expected_execution:None,schema_version: pab_protocol::FILE_MANAGER_SCHEMA_VERSION,request_id:id,execution: pab_protocol::ExecutionSelection::Service {},operation:operation.clone()};
        request.validate().map_err(FileManagerError::Invalid)?;
        // Read results are regenerable and already scoped by the directory
        // index/window. Persist mutation outcomes, not one full JSON journal
        // entry for every stat of every file in a large transfer tree.
        if !matches!(operation,Op::CreateDirectory {..}|Op::PrepareDirectory {..}|Op::Rename {..}|Op::Delete {..}|Op::SetDirectoryMetadata {..}) {
            if let Some(row)=sqlx::query("SELECT owner,spec,reply FROM local_manager_requests WHERE id=?").bind(id.to_string()).fetch_optional(&self.journal).await? {
                if row.try_get::<String,_>("owner")?!=owner{return Err(FileManagerError::NotFound);}
                if row.try_get::<String,_>("spec")?!=serde_json::to_string(operation)?{return Err(FileManagerError::Conflict);}
                if let Some(data)=row.try_get::<Option<String>,_>("reply")?{return Ok(serde_json::from_str(&data)?);}
            }
            let body=match self.perform(id,owner,operation,cancel).await {Ok((body,_))=>body,Err(error)=>Body::Error {error:pab_protocol::FileSystemError {code:error_code(&error).into(),phase:"local_files".into(),message:error.to_string()}}};
            return Ok(reply(id,body));
        }
        let spec = serde_json::to_string(operation)?;
        let mut tx = crate::begin_write_transaction(&self.journal,"UPDATE local_manager_requests SET created=created WHERE 0").await?;
        if let Some(row) = sqlx::query("SELECT owner,spec,reply FROM local_manager_requests WHERE id=?").bind(id.to_string()).fetch_optional(&mut *tx).await? {
            if row.try_get::<String,_>("owner")? != owner { return Err(FileManagerError::NotFound); }
            if row.try_get::<String,_>("spec")? != spec { return Err(FileManagerError::Conflict); }
            let cached=match row.try_get::<Option<String>,_>("reply")? { Some(data) => serde_json::from_str(&data)?, None => reply(id,Body::Error {error:pab_protocol::FileSystemError {code:"unconfirmed".into(),phase:"reconcile".into(),message:"The original local operation has no confirmed outcome; inspect its path before starting another operation.".into()}}) };
            if retain_receipt {sqlx::query("UPDATE local_manager_requests SET retained=1 WHERE id=? AND owner=?").bind(id.to_string()).bind(owner).execute(&mut *tx).await?;}
            tx.commit().await?;
            #[cfg(unix)]
            if matches!(operation,Op::PrepareDirectory {existing_object:None,..}) && matches!(&cached.result,Body::Error {error} if matches!(error.code.as_str(),"unconfirmed"|"outcome_unconfirmed")) {
                if let Some(recovered)=self.observe(owner,id,operation).await? {return Ok(recovered);}
            }
            return Ok(cached);
        }
        #[cfg(unix)]
        if matches!(operation,Op::PrepareDirectory {existing_object:None,..}) {
            let database=self.staging_database.clone();
            if matches!(blocking(move ||crate::observe_directory_creation(&database,id)).await?,crate::DirectoryCreationObservation::Recorded(_)|crate::DirectoryCreationObservation::Pending){return Err(FileManagerError::OutcomeUnconfirmed("original directory evidence exists without its admitted database record".into()));}
        }
        sqlx::query("DELETE FROM local_manager_requests WHERE created<? AND reply IS NOT NULL AND retained=0").bind(crate::now_ms()-86_400_000).execute(&mut *tx).await?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_manager_requests").fetch_one(&mut *tx).await?;
        if count >= 100_000 { return Err(FileManagerError::Capacity); }
        sqlx::query("INSERT INTO local_manager_requests(id,owner,spec,created,retained) VALUES(?,?,?,?,?)").bind(id.to_string()).bind(owner).bind(spec).bind(crate::now_ms()).bind(i64::from(retain_receipt)).execute(&mut *tx).await?;
        tx.commit().await?;
        let result = self.perform(id,owner,operation,cancel).await;
        let (body, retained) = match result {
            Ok(result) => result,
            Err(error) => (Body::Error {error:pab_protocol::FileSystemError {code: error_code(&error).into(),phase:"local_files".into(),message:error.to_string()}},None),
        };
        let result = reply(id,body);
        let changed=sqlx::query("UPDATE local_manager_requests SET reply=? WHERE id=? AND owner=?").bind(serde_json::to_string(&result)?).bind(id.to_string()).bind(owner).execute(&self.journal).await?;
        if changed.rows_affected()!=1 {return Err(FileManagerError::OutcomeUnconfirmed("local mutation result did not update its admitted record".into()));}
        if let Some(mut object)=retained {
            if let Err(error)=object.acknowledge_recorded(){eprintln!("file-manager: recorded directory receipt retained for later acknowledgement: {error}");}
        }
        Ok(result)
    }
    async fn perform(&self, _id:RequestId, owner:&str, operation:&Op, cancel:watch::Receiver<bool>) -> Result<(Body,Option<crate::DirectoryObject>),FileManagerError> {
        if *cancel.borrow() { return Err(FileManagerError::Interrupted); }
        if let Op::PrepareDirectory {path,expected_parent,existing_object}=operation {
            let _guard=self.operations.lock().await;
            let (path,parent,existing)=(path.clone(),expected_parent.clone(),existing_object.clone());
            #[cfg(unix)]
            let database=self.staging_database.clone();
            return blocking(move || {
                #[cfg(unix)]
                let object=crate::DirectoryObject::prepare_journaled(&database,_id,&path,&parent,existing.as_deref())?;
                #[cfg(not(unix))]
                let object=crate::DirectoryObject::prepare(&path,&parent,existing.as_deref())?;
                Ok((Body::DirectoryPrepared {path,identity:object.identity().into(),created:existing.is_none()},Some(object)))
            }).await;
        }
        if let Op::SetDirectoryMetadata {path,expected_object,metadata} = operation {
            let _guard = self.operations.lock().await;
            let (path,expected,metadata) = (path.clone(),expected_object.clone(),metadata.clone());
            return blocking(move || {
                let object = crate::DirectoryObject::open(&path,true)?;
                let metadata = object.apply(&expected,&metadata)?;
                Ok((Body::DirectoryMetadata {path,identity:expected,metadata},Some(object)))
            }).await;
        }
        Ok((match operation {
            Op::SetDirectoryMetadata {..}|Op::PrepareDirectory {..} => unreachable!(),
            Op::InspectPathIdentity {path} => {
                let path=path.clone();
                blocking(move || Ok(Body::PathIdentity {identity:crate::inspect_path_identity(&path)?,path})).await?
            },
            Op::InspectDirectoryObject {path} => {
                let path = path.clone();
                blocking(move || {
                    let object = crate::DirectoryObject::open(&path,false)?;
                    Ok(Body::DirectoryObject {path,identity:object.identity().into(),metadata:object.metadata()?})
                }).await?
            },
            Op::Locations {} => Body::Locations {locations: blocking(crate::locations).await?},
            Op::OpenDirectory {path} => Body::Opened {snapshot_id:self.index.scan(owner,path,cancel).await?},
            Op::ReadDirectory {snapshot_id,after,limit,filter} => Body::Directory {page:self.index.read(owner,*snapshot_id,after.as_deref(),*limit,filter).await?},
            Op::CloseDirectory {snapshot_id} => {self.index.close(owner,*snapshot_id).await?;Body::Closed {snapshot_id:*snapshot_id}},
            Op::Inspect {path} => Body::Entry {entry:crate::inspect(path).await?},
            Op::CreateDirectory {parent,name} => {
                let _guard = self.operations.lock().await;
                let (parent,name) = (parent.clone(),name.clone());
                Body::Applied {path:blocking(move || crate::create_directory_native(&parent,&name)).await?}
            },
            Op::Rename {path,name} => {
                let _guard = self.operations.lock().await;
                let (path,name) = (path.clone(),name.clone());
                Body::Applied {path:blocking(move || crate::rename_native(&path,&name)).await?}
            },
            Op::Delete {path,recursive} => {
                let _guard = self.operations.lock().await;
                let (path,recursive) = (path.clone(),*recursive);
                let deleted = path.clone();
                blocking(move || crate::delete(&path,recursive)).await?;
                Body::Applied {path:deleted}
            },
        },None))
    }
    pub async fn release(&self, owner:&str) -> Result<(),FileManagerError> {self.index.release_owner(owner).await}
}
fn error_code(error:&FileManagerError)->&'static str {
    match error {
        FileManagerError::Interrupted=>"cancelled",
        FileManagerError::Conflict=>"conflict",
        FileManagerError::NotFound=>"not_found",
        FileManagerError::OutcomeUnconfirmed(_)=>"outcome_unconfirmed",
        FileManagerError::SnapshotUnavailable=>"snapshot_unavailable",
        FileManagerError::Invalid(_)=>"invalid_path",
        FileManagerError::Capacity=>"resource_limit",
        FileManagerError::Io(error)=>match error.kind() {
            std::io::ErrorKind::NotFound=>"not_found",
            std::io::ErrorKind::AlreadyExists=>"already_exists",
            std::io::ErrorKind::PermissionDenied=>"access_denied",
            std::io::ErrorKind::DirectoryNotEmpty=>"directory_not_empty",
            _=>"filesystem_io",
        },
        _=>"file_manager_storage",
    }
}
async fn blocking<T:Send+'static>(task:impl FnOnce()->Result<T,FileManagerError>+Send+'static)->Result<T,FileManagerError> {
    tokio::task::spawn_blocking(task).await.map_err(|error| FileManagerError::Io(std::io::Error::other(error)))?
}
fn reply(id:RequestId,result:Body)->FileManagerReply {FileManagerReply {schema_version:pab_protocol::FILE_MANAGER_SCHEMA_VERSION,request_id:id,execution_context:None,result}}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn batch_receipt_remains_observable_past_ttl_without_rebinding_or_replaying() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let db=root.join("local.sqlite3");
        let manager=LocalFileManager::open(&db).await.unwrap();let browser=manager.browser("batch:retained".into());
        let expected_parent=crate::DirectoryObject::open(&crate::encode_path(&root),false).unwrap().identity().to_owned();
        let op=Op::PrepareDirectory {path:crate::encode_path(&root.join("selected")),expected_parent,existing_object:None};let id=RequestId::new();let (_send,cancel)=watch::channel(false);
        let receipt=browser.execute_request(id,op.clone(),cancel.clone()).await.unwrap();assert!(matches!(receipt.result,Body::DirectoryPrepared {..}));
        std::fs::rename(root.join("selected"),root.join("original")).unwrap();std::fs::create_dir(root.join("selected")).unwrap();std::fs::write(root.join("selected/sentinel"),b"replacement").unwrap();
        sqlx::query("UPDATE local_manager_requests SET created=? WHERE id=?").bind(crate::now_ms()-172_800_000).bind(id.to_string()).execute(&manager.journal).await.unwrap();
        manager.execute("ui",RequestId::new(),&Op::CreateDirectory {parent:crate::encode_path(&root),name:pab_protocol::NativeFilePath::Utf8("later".into())},cancel).await.unwrap();
        let reopened=LocalFileManager::open(&db).await.unwrap();let observed=reopened.browser("batch:retained".into()).observe_request(id,&op).await.expect("batch still needs the original receipt after 24 hours");
        assert_eq!(observed,Some(receipt));assert_eq!(std::fs::read(root.join("selected/sentinel")).unwrap(),b"replacement");
        manager.index.clone().shutdown().await;manager.journal.close().await;
        reopened.index.clone().shutdown().await;reopened.journal.close().await;
        temp.close().unwrap();assert!(!root.exists());
    }
    #[tokio::test]
    async fn directory_preparation_records_original_object_and_never_rebinds_on_reopen() {
        for merge in [false,true] {
            let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let parent=root.join("parent");std::fs::create_dir(&parent).unwrap();let child=parent.join("child");
            let expected_parent=crate::DirectoryObject::open(&crate::encode_path(&parent),false).unwrap().identity().to_owned();
            let existing_object=if merge {std::fs::create_dir(&child).unwrap();Some(crate::DirectoryObject::open(&crate::encode_path(&child),false).unwrap().identity().to_owned())}else{None};
            let operation=Op::PrepareDirectory {path:crate::encode_path(&child),expected_parent,existing_object};let id=RequestId::new();let (_send,cancel)=watch::channel(false);
            let db=root.join("local.sqlite3");let manager=LocalFileManager::open(&db).await.unwrap();let reply=manager.execute("owner",id,&operation,cancel.clone()).await.unwrap();
            assert!(matches!(&reply.result,Body::DirectoryPrepared {created,..} if *created==!merge),"{:?}",reply.result);
            std::fs::rename(&child,parent.join("original")).unwrap();std::fs::create_dir(&child).unwrap();std::fs::write(child.join("sentinel"),b"replacement").unwrap();
            let reopened=LocalFileManager::open(&db).await.unwrap();assert_eq!(reopened.execute("owner",id,&operation,cancel.clone()).await.unwrap(),reply);assert_eq!(reopened.observe("owner",id,&operation).await.unwrap(),Some(reply));
            assert!(reopened.observe("foreign",id,&operation).await.is_err());
            assert!(matches!(reopened.execute("owner",RequestId::new(),&operation,cancel).await.unwrap().result,Body::Error {..}));
            assert_eq!(std::fs::read(child.join("sentinel")).unwrap(),b"replacement");
        }
    }
    #[tokio::test]
    async fn directory_attribute_receipt_is_durable_and_never_reapplied_to_a_replacement() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
        let path=root.join("directory");std::fs::create_dir(&path).unwrap();
        let database=root.join("manager.sqlite3");let manager=LocalFileManager::open(&database).await.unwrap();
        let (_send,cancel)=watch::channel(false);
        let inspected=manager.execute("owner",RequestId::new(),&Op::InspectDirectoryObject {path:crate::encode_path(&path)},cancel.clone()).await.unwrap();
        let Body::DirectoryObject {identity,..}=inspected.result else {panic!("missing directory identity");};
        let op=Op::SetDirectoryMetadata {path:crate::encode_path(&path),expected_object:identity,metadata:pab_protocol::FileTransferMetadata {modified_at_unix_ms:Some(1_700_000_000_123),unix_mode:None}};
        let id=RequestId::new();let reply=manager.execute("owner",id,&op,cancel.clone()).await.unwrap();
        assert!(matches!(&reply.result,Body::DirectoryMetadata {metadata,..} if metadata.warnings.is_empty()));
        std::fs::rename(&path,root.join("original")).unwrap();std::fs::create_dir(&path).unwrap();
        let before=crate::modified_at_unix_ms(&std::fs::metadata(&path).unwrap());
        let restarted=LocalFileManager::open(&database).await.unwrap();
        assert_eq!(restarted.observe("owner",id,&op).await.unwrap(),Some(reply.clone()));
        assert_eq!(restarted.execute("owner",id,&op,cancel.clone()).await.unwrap(),reply);
        assert!(restarted.execute("different-owner",id,&op,cancel.clone()).await.is_err());
        assert!(matches!(restarted.execute("owner",RequestId::new(),&op,cancel.clone()).await.unwrap().result,Body::Error {..}));
        // Accepted but unrecorded work remains uncertain. Replaying its ID
        // cannot silently perform the attribute mutation again.
        sqlx::query("UPDATE local_manager_requests SET reply=NULL WHERE id=?").bind(id.to_string()).execute(&manager.journal).await.unwrap();
        assert!(restarted.observe("owner",id,&op).await.unwrap().is_none());
        assert!(matches!(restarted.execute("owner",id,&op,cancel).await.unwrap().result,Body::Error {error} if error.code=="unconfirmed"));
        assert_eq!(crate::modified_at_unix_ms(&std::fs::metadata(path).unwrap()),before);
    }
    #[tokio::test]
    async fn deleted_objects_are_not_deleted_again_after_lost_reply_or_restart() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let database = root.join("manager.sqlite3");
        let manager = LocalFileManager::open(&database).await.unwrap();
        let path = root.join("selected");
        std::fs::write(&path,b"old").unwrap();
        let (_send,probe)=watch::channel(false);
        let absent=manager.execute("first",RequestId::new(),&Op::Inspect {path:crate::encode_path(&root.join("not-present"))},probe).await.unwrap();
        assert!(matches!(absent.result,Body::Error {error} if error.code=="not_found"));
        let read_journal:i64=sqlx::query_scalar("SELECT COUNT(*) FROM local_manager_requests").fetch_one(&manager.journal).await.unwrap();assert_eq!(read_journal,0,"regenerable metadata must not consume the durable mutation quota");
        let op = Op::Delete {path:crate::encode_path(&path),recursive:false};
        let id = RequestId::new();
        let (_send,cancel) = watch::channel(false);
        let result = manager.execute("first",id,&op,cancel.clone()).await.unwrap();
        assert!(matches!(result.result,Body::Applied {..}));
        std::fs::write(&path,b"new").unwrap();
        let restarted = LocalFileManager::open(&database).await.unwrap();
        assert_eq!(restarted.execute("first",id,&op,cancel.clone()).await.unwrap(),result);
        assert_eq!(restarted.observe("first",id,&op).await.unwrap(),Some(result));
        assert!(restarted.observe("other",id,&op).await.is_err());
        assert_eq!(std::fs::read(&path).unwrap(),b"new");
        assert!(restarted.execute("other",id,&op,cancel.clone()).await.is_err());
        assert!(restarted.execute("first",id,&Op::Inspect {path:crate::encode_path(&path)},cancel.clone()).await.is_err());
        sqlx::query("UPDATE local_manager_requests SET reply=NULL WHERE id=?").bind(id.to_string()).execute(&manager.journal).await.unwrap();
        assert!(restarted.observe("first",id,&op).await.unwrap().is_none());
        assert!(matches!(restarted.execute("first",id,&op,cancel).await.unwrap().result,Body::Error {error} if error.code == "unconfirmed"));
        assert_eq!(std::fs::read(path).unwrap(),b"new");
    }
}
