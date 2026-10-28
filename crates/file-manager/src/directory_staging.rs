//! Per-native-user process-recovery journal for private directory staging.
//! A locked, append-only receipt precedes side effects. Recovery uses recorded
//! object identities and empty-directory removal only, never a prefix sweep.
use crate::{FileManagerError, decode_path, encode_path, mutation::open_directory};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, DirBuilder, DirBuilderExt, OpenOptions, OpenOptionsExt};
use pab_protocol::{NativeFilePath,RequestId};
use serde::{Deserialize, Serialize};
use std::{ffi::OsStr, fs::File, io::{self, Read, Seek, SeekFrom, Write}, os::unix::fs::MetadataExt, path::Path};

const MAX_RECORD_BYTES:u64=128*1024;
const MAX_RECORDS:usize=1024;

#[derive(Debug, Clone, Copy, Default)]
pub struct DirectoryStagingRecovery {pub cleaned:u32,pub busy:u32,pub retained:u32}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version:u8,
    request_id:RequestId,
    parent:NativeFilePath,
    parent_identity:String,
    staging_name:String,
    private_identity:Option<String>,
    entry_identity:Option<String>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    target:Option<NativeFilePath>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    published_identity:Option<String>,
}

struct Journal {directory:Dir}
impl Journal {
    fn open(database:&Path,create:bool)->Result<Option<Self>,FileManagerError> {
        let root=database.with_extension("directory-staging");
        let parent=root.parent().ok_or(FileManagerError::Invalid("missing staging journal parent"))?;
        let name=root.file_name().ok_or(FileManagerError::Invalid("missing staging journal name"))?;
        let parent=open_directory(parent)?;
        if create {
            let mut builder=DirBuilder::new();builder.mode(0o700);
            match parent.create_dir_with(name,&builder) {Ok(())=>{},Err(error) if error.kind()==io::ErrorKind::AlreadyExists=>{},Err(error)=>return Err(error.into())}
        }
        let directory=match parent.open_dir_nofollow(name) {Ok(dir)=>dir,Err(error) if !create && error.kind()==io::ErrorKind::NotFound=>return Ok(None),Err(error)=>return Err(error.into())};
        pab_os_control::filesystem::verify_private_directory(&directory.try_clone()?.into_std_file())?;
        Ok(Some(Self {directory}))
    }
    fn options()->OpenOptions {let mut options=OpenOptions::new();options.read(true).write(true).mode(0o600).custom_flags(pab_os_control::filesystem::NONBLOCKING_FILE_OPEN_FLAGS).follow(FollowSymlinks::No);options}
    fn valid_file(&self,file:&File)->io::Result<()> {
        let metadata=file.metadata()?;
        if !metadata.is_file() || metadata.uid()!=self.directory.try_clone()?.into_std_file().metadata()?.uid() || metadata.mode()&0o077!=0 || pab_os_control::filesystem::file_link_count(file)?!=1 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied,"staging journal is not a private ordinary file"));
        }
        Ok(())
    }
    fn begin(&self,request_id:RequestId,parent:&NativeFilePath,parent_identity:&str)->Result<Guard,FileManagerError> {
        self.begin_bound(request_id,parent,parent_identity,None)
    }
    fn begin_bound(&self,request_id:RequestId,parent:&NativeFilePath,parent_identity:&str,target:Option<NativeFilePath>)->Result<Guard,FileManagerError> {
        let nonce=RequestId::new();let name=format!("{nonce}.jsonl");
        let mut options=Self::options();options.create_new(true);
        let file=self.directory.open_with(&name,&options)?.into_std();
        self.valid_file(&file)?;
        file.try_lock().map_err(io::Error::other)?;
        let record=Record {version:if target.is_some(){2}else{1},request_id,parent:parent.clone(),parent_identity:parent_identity.into(),staging_name:format!(".pab-dir-{nonce}"),private_identity:None,entry_identity:None,target,published_identity:None};
        let mut guard=Guard {journal:Self {directory:self.directory.try_clone()?},name,file,record,keep_until_ack:false};
        guard.append()?;
        // Durability here is for process death. No separate power-loss
        // guarantee is inferred from filesystems that cannot fsync a directory.
        pab_os_control::filesystem::sync_directory_handle(&self.directory.try_clone()?.into_std_file())?;
        Ok(guard)
    }
    fn recover(&self)->Result<DirectoryStagingRecovery,FileManagerError> {
        let mut report=DirectoryStagingRecovery::default();
        for (index,entry) in self.directory.entries()?.enumerate() {
            if index>=MAX_RECORDS {return Err(FileManagerError::Capacity);}
            let entry=entry?;let name=entry.file_name();
            if journal_nonce(&name).is_none() {report.retained+=1;continue;}
            let mut file=match self.directory.open_with(&name,&Self::options()) {Ok(file)=>file.into_std(),Err(_)=>{report.retained+=1;continue}};
            if self.valid_file(&file).is_err() {report.retained+=1;continue;}
            if file.try_lock().is_err() {report.busy+=1;continue;}
            match self.clean(&name,&mut file,false) {Ok(())=>report.cleaned+=1,Err(_)=>report.retained+=1}
        }
        Ok(report)
    }
    fn read(&self,name:&OsStr,file:&mut File)->Result<Record,FileManagerError> {
        let nonce=journal_nonce(name).ok_or(FileManagerError::Invalid("unowned staging journal"))?;
        self.valid_file(file)?;
        file.seek(SeekFrom::Start(0))?;
        let mut bytes=Vec::new();(&mut *file).take(MAX_RECORD_BYTES+1).read_to_end(&mut bytes)?;
        if bytes.len() as u64>MAX_RECORD_BYTES {return Err(FileManagerError::Capacity);}
        let last=bytes.iter().rposition(|byte|*byte==b'\n').ok_or(FileManagerError::Invalid("staging journal has no complete intent"))?;
        let mut original:Option<Record>=None;let mut previous:Option<Record>=None;
        for (index,line) in bytes[..last].split(|byte|*byte==b'\n').enumerate() {
            if index>=4 {return Err(FileManagerError::Invalid("too many staging checkpoints"));}
            let next:Record=serde_json::from_slice(line)?;
            next.parent.validate().map_err(FileManagerError::Invalid)?;
            pab_protocol::FileTransferParentConstraint {identity:next.parent_identity.clone()}.validate().map_err(FileManagerError::Invalid)?;
            if !matches!(next.version,1|2) || next.staging_name!=format!(".pab-dir-{nonce}") || next.entry_identity.is_some() && next.private_identity.is_none() {return Err(FileManagerError::Conflict);}
            if next.version==1 && (next.target.is_some() || next.published_identity.is_some()) {return Err(FileManagerError::Conflict);}
            if let Some(target)=&next.target {
                target.validate().map_err(FileManagerError::Invalid)?;
                let path=decode_path(target)?;
                if path.parent()!=Some(decode_path(&next.parent)?.as_path()) {return Err(FileManagerError::Conflict);}
                crate::decode_component(&encode_path(Path::new(path.file_name().ok_or(FileManagerError::Conflict)?)))?;
            }else if next.version==2 {return Err(FileManagerError::Conflict);}
            if let Some(identity)=&next.published_identity {
                if next.entry_identity.as_ref()!=Some(identity) || next.target.is_none() {return Err(FileManagerError::Conflict);}
                pab_protocol::FileTransferParentConstraint {identity:identity.clone()}.validate().map_err(FileManagerError::Invalid)?;
            }
            if let Some(first)=&original {
                let prior=previous.as_ref().unwrap();
                if next.version!=first.version || next.target!=first.target || next.request_id!=first.request_id || next.parent!=first.parent || next.parent_identity!=first.parent_identity || next.staging_name!=first.staging_name || prior.private_identity.as_ref().is_some_and(|old|next.private_identity.as_ref()!=Some(old)) || prior.entry_identity.as_ref().is_some_and(|old|next.entry_identity.as_ref()!=Some(old)) || prior.published_identity.is_some() {return Err(FileManagerError::Conflict);}
            }else {
                if next.private_identity.is_some() || next.entry_identity.is_some() || next.published_identity.is_some() {return Err(FileManagerError::Conflict);}
                original=Some(next.clone());
            }
            previous=Some(next);
        }
        previous.ok_or(FileManagerError::Invalid("empty staging journal"))
    }
    fn clean(&self,name:&OsStr,file:&mut File,acknowledged:bool)->Result<(),FileManagerError> {
        let record=self.read(name,file)?;
        // Once a v2 entry has been checkpointed it could have been published.
        // A crash before its final checkpoint is unknown, not proof of failure.
        if !acknowledged && record.version==2 && record.entry_identity.is_some() {return Err(FileManagerError::Interrupted);}
        let parent=open_directory(&decode_path(&record.parent)?)?.into_std_file();
        if pab_os_control::filesystem::file_object_identity(&parent)?!=record.parent_identity {return Err(FileManagerError::Conflict);}
        pab_os_control::filesystem::cleanup_directory_staging_at(&parent,OsStr::new(&record.staging_name),record.private_identity.as_deref(),record.entry_identity.as_deref())?;
        // The journal directory is private to this native UID; do not unlink a
        // replaced or hard-linked receipt even when its old descriptor is valid.
        let named=self.directory.open_with(name,&Self::options())?.into_std();
        self.valid_file(&named)?;
        if pab_os_control::filesystem::file_object_identity(&named)?!=pab_os_control::filesystem::file_object_identity(file)? {return Err(FileManagerError::Conflict);}
        self.directory.remove_file(name)?;
        Ok(())
    }
}
fn journal_nonce(name:&OsStr)->Option<RequestId> {
    let value=name.to_str()?.strip_suffix(".jsonl")?;
    let id=value.parse::<RequestId>().ok()?;
    (id.to_string()==value).then_some(id)
}
pub(crate) struct Guard {journal:Journal,name:String,file:File,record:Record,keep_until_ack:bool}
impl Guard {
    fn append(&mut self)->Result<(),FileManagerError> {
        let data=serde_json::to_vec(&self.record)?;
        if self.file.metadata()?.len()+data.len() as u64+1>MAX_RECORD_BYTES {return Err(FileManagerError::Capacity);}
        self.file.seek(SeekFrom::End(0))?;self.file.write_all(&data)?;self.file.write_all(b"\n")?;self.file.sync_all()?;Ok(())
    }
    fn checkpoint(&mut self,private:&File,entry:Option<&File>)->io::Result<()> {
        self.record.private_identity=Some(pab_os_control::filesystem::file_object_identity(private)?);
        self.record.entry_identity=entry.map(pab_os_control::filesystem::file_object_identity).transpose()?;
        self.append().map_err(io::Error::other)
    }
    pub(crate) fn acknowledge(mut self)->Result<(),FileManagerError> {
        self.journal.clean(OsStr::new(&self.name),&mut self.file,true)?;
        self.keep_until_ack=false;
        Ok(())
    }
}
impl Drop for Guard {
    fn drop(&mut self) {if !self.keep_until_ack {let _=self.journal.clean(OsStr::new(&self.name),&mut self.file,true);}}
}

/// Recover the current native user's inactive records. Busy records are not
/// errors. A missing checkpoint, changed object/parent, unexpected contents or
/// unreadable receipt is retained for diagnosis rather than guessed or deleted.
pub fn recover_directory_staging(database:&Path)->Result<DirectoryStagingRecovery,FileManagerError> {
    match Journal::open(database,false)? {Some(journal)=>journal.recover(),None=>Ok(Default::default())}
}

pub(crate) fn create_tracked(database:&Path,request:RequestId,parent:&File,parent_path:&Path,name:&OsStr,expected_parent:&str)->Result<(File,Guard),FileManagerError> {
    let journal=Journal::open(database,true)?.unwrap();
    let report=journal.recover()?;
    if report.busy as usize+report.retained as usize>=MAX_RECORDS {return Err(FileManagerError::Capacity);}
    if report.retained>0 {eprintln!("file-manager: {} inactive directory staging receipts retained for diagnosis",report.retained);}
    let mut guard=journal.begin_bound(request,&encode_path(parent_path),expected_parent,Some(encode_path(&parent_path.join(name))))?;
    let staging=guard.record.staging_name.clone();
    let file=pab_os_control::filesystem::create_directory_at_tracked(parent,name,OsStr::new(&staging),|private,entry|guard.checkpoint(private,entry))?;
    guard.keep_until_ack=true;
    guard.record.published_identity=Some(pab_os_control::filesystem::file_object_identity(&file).map_err(|error|FileManagerError::OutcomeUnconfirmed(error.to_string()))?);
    guard.append().map_err(|error|FileManagerError::OutcomeUnconfirmed(error.to_string()))?;
    Ok((file,guard))
}

/// A locked original publication fact, not a fresh inspection of its pathname.
/// Callers must verify ownership in their task database before using it and
/// acknowledge only after the exact operation/result has been saved there.
pub struct DirectoryCreationReceipt {pub(crate) fact:crate::FileMutationReceipt,guard:Guard}
pub enum DirectoryCreationObservation {Absent,Unavailable,Pending,Recorded(DirectoryCreationReceipt)}
impl DirectoryCreationReceipt {
    pub fn fact(&self)->&crate::FileMutationReceipt {&self.fact}
    pub fn acknowledge(self)->Result<(),FileManagerError> {self.guard.acknowledge()}
}
/// Absence is confirmed only when every possible record can be inspected under
/// its lock. A live or damaged record cannot be mistaken for a released receipt.
pub fn observe_directory_creation(database:&Path,id:RequestId)->Result<DirectoryCreationObservation,FileManagerError> {
    let Some(journal)=Journal::open(database,false)? else{return Ok(DirectoryCreationObservation::Absent);};
    let mut candidate=None;let mut uncertain=false;let mut matching=false;
    for (index,entry) in journal.directory.entries()?.enumerate() {
        if index>=MAX_RECORDS{return Err(FileManagerError::Capacity);}
        let name=entry?.file_name();if journal_nonce(&name).is_none(){continue;}
        let mut file=match journal.directory.open_with(&name,&Journal::options()){Ok(file)=>file.into_std(),Err(_)=>{uncertain=true;continue;}};
        if journal.valid_file(&file).is_err() || file.try_lock().is_err(){uncertain=true;continue;}
        let record=match journal.read(&name,&mut file){Ok(record)=>record,Err(_)=>{uncertain=true;continue;}};
        if record.request_id!=id {continue;}
        if matching{return Err(FileManagerError::Conflict);}matching=true;
        let (Some(path),Some(identity))=(&record.target,&record.published_identity) else{uncertain=true;continue;};
        let fact=crate::FileMutationReceipt {request_id:id,operation:pab_protocol::FileManagerOperation::PrepareDirectory {path:path.clone(),expected_parent:record.parent_identity.clone(),existing_object:None},result:pab_protocol::FileManagerResult::DirectoryPrepared {path:path.clone(),identity:identity.clone(),created:true}};
        fact.validate()?;
        candidate=Some(DirectoryCreationReceipt {fact,guard:Guard {journal:Journal {directory:journal.directory.try_clone()?},name:name.to_str().ok_or(FileManagerError::Conflict)?.into(),file,record,keep_until_ack:true}});
    }
    if let Some(receipt)=candidate {Ok(DirectoryCreationObservation::Recorded(receipt))}else if matching {Ok(DirectoryCreationObservation::Pending)}else if uncertain {Ok(DirectoryCreationObservation::Unavailable)}else{Ok(DirectoryCreationObservation::Absent)}
}
pub fn directory_creation_receipts(database:&Path,after:Option<RequestId>)->Result<Vec<DirectoryCreationReceipt>,FileManagerError> {
    let Some(journal)=Journal::open(database,false)? else{return Ok(Vec::new());};
    let mut facts=Vec::new();let mut ids=std::collections::HashSet::new();
    for (index,entry) in journal.directory.entries()?.enumerate() {
        if index>=MAX_RECORDS {return Err(FileManagerError::Capacity);}
        let name=entry?.file_name();if journal_nonce(&name).is_none(){continue;}
        let mut file=match journal.directory.open_with(&name,&Journal::options()){Ok(file)=>file.into_std(),Err(_)=>continue};
        if journal.valid_file(&file).is_err() || file.try_lock().is_err(){continue;}
        let record=match journal.read(&name,&mut file){Ok(record)=>record,Err(_)=>continue};
        if after.is_some_and(|after|record.request_id.as_uuid()<=after.as_uuid()){continue;}
        let (Some(path),Some(identity))=(&record.target,&record.published_identity) else{continue;};
        let fact=crate::FileMutationReceipt {request_id:record.request_id,operation:pab_protocol::FileManagerOperation::PrepareDirectory {path:path.clone(),expected_parent:record.parent_identity.clone(),existing_object:None},result:pab_protocol::FileManagerResult::DirectoryPrepared {path:path.clone(),identity:identity.clone(),created:true}};
        fact.validate()?;
        if !ids.insert(record.request_id) {return Err(FileManagerError::Conflict);}
        let guard=Guard {journal:Journal {directory:journal.directory.try_clone()?},name:name.to_str().ok_or(FileManagerError::Conflict)?.into(),file,record,keep_until_ack:true};
        facts.push(DirectoryCreationReceipt {fact,guard});
        // Keep the first page in request-ID order without holding a descriptor
        // for every retained journal at once. Unselected records stay untouched.
        facts.sort_by_key(|receipt|receipt.fact.request_id.as_uuid());
        if facts.len()>128 {facts.pop();}
    }
    Ok(facts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::{PermissionsExt,symlink},process::{Child,Command,Stdio},time::{Duration,Instant}};

    fn setup()->(tempfile::TempDir,std::path::PathBuf,std::path::PathBuf) {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let db=root.join("index.sqlite3");(temp,root,db)
    }
    fn close_fixture(temp:tempfile::TempDir,root:&Path) {
        temp.close().expect("owned directory-staging fixture cleanup failed");
        assert!(!root.exists(),"owned directory-staging fixture root remained");
    }
    fn fixture(db:&Path,parent:&Path,entry:bool)->(Guard,std::path::PathBuf) {
        let journal=Journal::open(db,true).unwrap().unwrap();
        let parent_file=File::open(parent).unwrap();let identity=pab_os_control::filesystem::file_object_identity(&parent_file).unwrap();
        let mut guard=journal.begin(RequestId::new(),&encode_path(parent),&identity).unwrap();
        let stage=parent.join(&guard.record.staging_name);
        let mut builder=std::fs::DirBuilder::new();std::os::unix::fs::DirBuilderExt::mode(&mut builder,0o700);builder.create(&stage).unwrap();
        let private=File::open(&stage).unwrap();guard.checkpoint(&private,None).unwrap();
        if entry {std::fs::create_dir(stage.join("entry")).unwrap();let child=File::open(stage.join("entry")).unwrap();guard.checkpoint(&private,Some(&child)).unwrap();}
        (guard,stage)
    }
    #[test]
    fn publication_receipts_page_in_stable_order_and_release_only_after_acknowledgement() {
        let (temp,root,db)=setup();let parent=File::open(&root).unwrap();let identity=pab_os_control::filesystem::file_object_identity(&parent).unwrap();
        for index in (1..=130).rev(){
            let object=crate::DirectoryObject::prepare_journaled(&db,RequestId::from_u128(index),&encode_path(&root.join(format!("dir-{index}"))),&identity,None).unwrap();drop(object);
        }
        let first=directory_creation_receipts(&db,None).unwrap();assert_eq!(first.len(),128);
        assert_eq!(first.first().unwrap().fact.request_id,RequestId::from_u128(1));
        let after=first.last().unwrap().fact.request_id;assert_eq!(after,RequestId::from_u128(128));
        assert_eq!(recover_directory_staging(&db).unwrap().busy,128);
        for receipt in first {receipt.acknowledge().unwrap();}
        let second=directory_creation_receipts(&db,Some(after)).unwrap();assert_eq!(second.len(),2);
        assert_eq!(second.iter().map(|r|r.fact.request_id).collect::<Vec<_>>(),vec![RequestId::from_u128(129),RequestId::from_u128(130)]);
        for receipt in second {receipt.acknowledge().unwrap();}
        assert!(directory_creation_receipts(&db,None).unwrap().is_empty());
        assert_eq!(std::fs::read_dir(db.with_extension("directory-staging")).unwrap().count(),0);
        for index in 1..=130 {assert!(root.join(format!("dir-{index}")).is_dir());}
        close_fixture(temp,&root);
    }
    #[test]
    fn a_published_directory_keeps_its_original_receipt_until_result_recording_is_acknowledged() {
        let (temp,root,db)=setup();let parent=File::open(&root).unwrap();
        let identity=pab_os_control::filesystem::file_object_identity(&parent).unwrap();
        let path=encode_path(&root.join("public"));
        let object=crate::DirectoryObject::prepare_journaled(&db,RequestId::new(),&path,&identity,None).unwrap();
        let report=recover_directory_staging(&db).unwrap();
        assert_eq!((report.cleaned,report.busy,report.retained),(0,1,0),"native return must not release the receipt before the database records the outcome");
        drop(object);
        let report=recover_directory_staging(&db).unwrap();
        assert_eq!((report.cleaned,report.busy,report.retained),(0,0,1),"dropping an unacknowledged publication must preserve its original fact");
        close_fixture(temp,&root);
    }
    #[tokio::test]
    async fn a_zero_row_result_write_keeps_the_receipt_and_cannot_readmit_the_same_request() {
        let (temp,root,db)=setup();let manager=crate::LocalFileManager::open(&db).await.unwrap();
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&db)).await.unwrap();
        let identity=pab_os_control::filesystem::file_object_identity(&File::open(&root).unwrap()).unwrap();let id=RequestId::new();let path=encode_path(&root.join("public"));
        let operation=pab_protocol::FileManagerOperation::PrepareDirectory {path:path.clone(),expected_parent:identity,existing_object:None};
        sqlx::query("CREATE TRIGGER discard_result BEFORE UPDATE OF reply ON local_manager_requests BEGIN DELETE FROM local_manager_requests WHERE id=OLD.id; SELECT RAISE(IGNORE); END").execute(&pool).await.unwrap();
        let (_send,cancel)=tokio::sync::watch::channel(false);
        assert!(matches!(manager.execute("owner",id,&operation,cancel.clone()).await,Err(FileManagerError::OutcomeUnconfirmed(_))));
        assert_eq!(recover_directory_staging(&db).unwrap().retained,1);
        std::fs::rename(root.join("public"),root.join("original")).unwrap();std::fs::create_dir(root.join("public")).unwrap();std::fs::write(root.join("public/sentinel"),b"replacement").unwrap();
        for owner in ["owner","other"] {assert!(matches!(manager.execute(owner,id,&operation,cancel.clone()).await,Err(FileManagerError::OutcomeUnconfirmed(_))));}
        let rows:i64=sqlx::query_scalar("SELECT COUNT(*) FROM local_manager_requests WHERE id=?").bind(id.to_string()).fetch_one(&pool).await.unwrap();assert_eq!(rows,0);
        assert_eq!(std::fs::read(root.join("public/sentinel")).unwrap(),b"replacement");assert_eq!(recover_directory_staging(&db).unwrap().retained,1);
        pool.close().await;manager.close_for_test().await;drop(manager);close_fixture(temp,&root);
    }
    #[test]
    fn a_busy_or_incomplete_record_cannot_be_used_as_proof_of_absence() {
        let (temp,root,db)=setup();let parent=File::open(&root).unwrap();let identity=pab_os_control::filesystem::file_object_identity(&parent).unwrap();let id=RequestId::new();let other=RequestId::new();
        let mut object=crate::DirectoryObject::prepare_journaled(&db,id,&encode_path(&root.join("public")),&identity,None).unwrap();
        assert!(matches!(observe_directory_creation(&db,id).unwrap(),DirectoryCreationObservation::Unavailable));
        assert!(matches!(observe_directory_creation(&db,other).unwrap(),DirectoryCreationObservation::Unavailable));
        object.acknowledge_recorded().unwrap();drop(object);
        assert!(matches!(observe_directory_creation(&db,id).unwrap(),DirectoryCreationObservation::Absent));
        let (guard,stage)=fixture(&db,&root,true);let pending=guard.record.request_id;
        std::fs::write(stage.join("entry/sentinel"),b"keep inactive intent").unwrap();drop(guard);
        assert!(matches!(observe_directory_creation(&db,pending).unwrap(),DirectoryCreationObservation::Pending));
        assert!(matches!(observe_directory_creation(&db,other).unwrap(),DirectoryCreationObservation::Absent));
        close_fixture(temp,&root);
    }
    #[tokio::test]
    async fn a_failed_database_commit_recovers_the_original_publication_without_inspecting_its_current_name() {
        let (temp,root,db)=setup();let manager=crate::LocalFileManager::open(&db).await.unwrap();
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&db)).await.unwrap();
        let parent_identity=pab_os_control::filesystem::file_object_identity(&File::open(&root).unwrap()).unwrap();
        let path=encode_path(&root.join("public"));let id=RequestId::new();
        let operation=pab_protocol::FileManagerOperation::PrepareDirectory {path:path.clone(),expected_parent:parent_identity,existing_object:None};
        sqlx::query("CREATE TRIGGER reject_result BEFORE UPDATE OF reply ON local_manager_requests WHEN NEW.reply IS NOT NULL BEGIN SELECT RAISE(ABORT, 'injected result commit failure'); END").execute(&pool).await.unwrap();
        let (_send,cancel)=tokio::sync::watch::channel(false);
        assert!(matches!(manager.execute("owner",id,&operation,cancel.clone()).await,Err(FileManagerError::Database(_))));
        assert_eq!(recover_directory_staging(&db).unwrap().retained,1);
        let original=crate::DirectoryObject::open(&path,false).unwrap().identity().to_owned();
        std::fs::rename(root.join("public"),root.join("original")).unwrap();std::fs::create_dir(root.join("public")).unwrap();std::fs::write(root.join("public/sentinel"),b"replacement").unwrap();
        assert!(matches!(manager.observe("foreign",id,&operation).await,Err(FileManagerError::NotFound)));
        sqlx::query("DROP TRIGGER reject_result").execute(&pool).await.unwrap();
        let reopened=crate::LocalFileManager::open(&db).await.unwrap();
        let fact=reopened.observe("owner",id,&operation).await.unwrap().unwrap();
        assert_eq!(fact.result,pab_protocol::FileManagerResult::DirectoryPrepared {path:path.clone(),identity:original,created:true});
        assert_eq!(reopened.execute("owner",id,&operation,cancel).await.unwrap(),fact);
        assert_eq!(std::fs::read(root.join("public/sentinel")).unwrap(),b"replacement");
        assert_eq!(std::fs::read_dir(root.join("original")).unwrap().count(),0);
        assert_eq!(std::fs::read_dir(db.with_extension("directory-staging")).unwrap().count(),0);
        pool.close().await;manager.close_for_test().await;reopened.close_for_test().await;
        close_fixture(temp,&root);
    }
    #[tokio::test]
    async fn killed_publication_is_recovered_only_after_its_complete_original_checkpoint() {
        use sqlx::Row;
        for phase in ["after-entry","after-publication"] {
            let (temp,root,db)=setup();let manager=crate::LocalFileManager::open(&db).await.unwrap();let id=RequestId::new();
            let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&db)).await.unwrap();
            let identity=pab_os_control::filesystem::file_object_identity(&File::open(&root).unwrap()).unwrap();let path=encode_path(&root.join("public"));
            let operation=pab_protocol::FileManagerOperation::PrepareDirectory {path:path.clone(),expected_parent:identity,existing_object:None};
            sqlx::query("INSERT INTO local_manager_requests(id,owner,spec,created,retained) VALUES(?,?,?,?,1)").bind(id.to_string()).bind("owner").bind(serde_json::to_string(&operation).unwrap()).bind(crate::now_ms()).execute(&pool).await.unwrap();
            let mut child=OwnedChild(Command::new(std::env::current_exe().unwrap()).args(["--ignored","--exact","directory_staging::tests::publication_crash_child","--nocapture"]).env("PAB_DIRECTORY_CRASH_ROOT",&root).env("PAB_DIRECTORY_CRASH_PHASE",phase).env("PAB_DIRECTORY_CRASH_ID",id.to_string()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
            let deadline=Instant::now()+Duration::from_secs(15);
            while !root.join("ready").exists(){assert!(child.0.try_wait().unwrap().is_none());assert!(Instant::now()<deadline);std::thread::sleep(Duration::from_millis(20));}
            assert_eq!(recover_directory_staging(&db).unwrap().busy,1);
            child.0.kill().unwrap();assert!(!child.0.wait().unwrap().success());drop(child);
            let original=if phase=="after-publication" {let identity=crate::DirectoryObject::open(&path,false).unwrap().identity().to_owned();std::fs::rename(root.join("public"),root.join("original")).unwrap();Some(identity)}else{None};
            std::fs::create_dir(root.join("public")).unwrap();std::fs::write(root.join("public/sentinel"),b"replacement").unwrap();
            let reopened=crate::LocalFileManager::open(&db).await.unwrap();let observed=reopened.observe("owner",id,&operation).await.unwrap();
            if let Some(identity)=original {
                assert_eq!(observed.unwrap().result,pab_protocol::FileManagerResult::DirectoryPrepared {path,identity,created:true});
                assert_eq!(recover_directory_staging(&db).unwrap().retained,0);
            }else {
                assert!(observed.is_none());assert_eq!(recover_directory_staging(&db).unwrap().retained,1);
                let row=sqlx::query("SELECT reply FROM local_manager_requests WHERE id=?").bind(id.to_string()).fetch_one(&pool).await.unwrap();assert!(row.try_get::<Option<String>,_>("reply").unwrap().is_none());
            }
            assert!(matches!(reopened.observe("foreign",id,&operation).await,Err(FileManagerError::NotFound)));
            assert_eq!(std::fs::read(root.join("public/sentinel")).unwrap(),b"replacement");
            pool.close().await;manager.close_for_test().await;reopened.close_for_test().await;close_fixture(temp,&root);
        }
    }
    #[test]
    #[ignore]
    fn publication_crash_child() {
        let root=std::path::PathBuf::from(std::env::var_os("PAB_DIRECTORY_CRASH_ROOT").unwrap());let phase=std::env::var("PAB_DIRECTORY_CRASH_PHASE").unwrap();let id=std::env::var("PAB_DIRECTORY_CRASH_ID").unwrap().parse().unwrap();let db=root.join("index.sqlite3");
        let wait=||->! {std::fs::write(root.join("ready"),b"ready").unwrap();loop{std::thread::park_timeout(Duration::from_secs(30));}};
        let parent=File::open(&root).unwrap();let identity=pab_os_control::filesystem::file_object_identity(&parent).unwrap();
        if phase=="after-entry" {
            let journal=Journal::open(&db,true).unwrap().unwrap();let mut guard=journal.begin(id,&encode_path(&root),&identity).unwrap();
            guard.record.version=2;guard.record.target=Some(encode_path(&root.join("public")));guard.file.set_len(0).unwrap();guard.append().unwrap();
            let name=guard.record.staging_name.clone();let _=pab_os_control::filesystem::create_directory_at_tracked(&parent,OsStr::new("public"),OsStr::new(&name),|private,entry|{guard.checkpoint(private,entry)?;if entry.is_some(){wait();}Ok(())});
            unreachable!();
        }
        let _object=crate::DirectoryObject::prepare_journaled(&db,id,&encode_path(&root.join("public")),&identity,None).unwrap();wait();
    }
    #[test]
    fn successful_and_failed_creation_cleanup_is_idempotent_and_never_removes_public_target() {
        let (_temp,root,db)=setup();let parent=File::open(&root).unwrap();let identity=pab_os_control::filesystem::file_object_identity(&parent).unwrap();
        let path=encode_path(&root.join("public"));
        let mut object=crate::DirectoryObject::prepare_journaled(&db,RequestId::new(),&path,&identity,None).unwrap();
        assert_eq!(object.identity(),crate::DirectoryObject::open(&path,false).unwrap().identity());
        assert_eq!(recover_directory_staging(&db).unwrap().busy,1);
        object.acknowledge_recorded().unwrap();
        assert_eq!(std::fs::read_dir(db.with_extension("directory-staging")).unwrap().count(),0);
        std::fs::write(root.join("public/sentinel"),b"published").unwrap();
        assert!(crate::DirectoryObject::prepare_journaled(&db,RequestId::new(),&path,&identity,None).is_err());
        for _ in 0..2 {let report=recover_directory_staging(&db).unwrap();assert_eq!((report.cleaned,report.busy,report.retained),(0,0,0));}
        assert_eq!(std::fs::read(root.join("public/sentinel")).unwrap(),b"published");
        close_fixture(_temp,&root);
    }
    #[test]
    fn a_shared_or_symlinked_journal_root_is_rejected_before_creation() {
        let (_temp,root,db)=setup();let journal_root=db.with_extension("directory-staging");
        std::fs::create_dir(&journal_root).unwrap();std::fs::set_permissions(&journal_root,std::fs::Permissions::from_mode(0o777)).unwrap();
        let identity=pab_os_control::filesystem::file_object_identity(&File::open(&root).unwrap()).unwrap();let path=encode_path(&root.join("public"));
        assert!(crate::DirectoryObject::prepare_journaled(&db,RequestId::new(),&path,&identity,None).is_err());assert!(!root.join("public").exists());
        std::fs::remove_dir(&journal_root).unwrap();let outside=root.join("outside");std::fs::create_dir(&outside).unwrap();symlink(&outside,&journal_root).unwrap();
        assert!(crate::DirectoryObject::prepare_journaled(&db,RequestId::new(),&path,&identity,None).is_err());assert!(!root.join("public").exists());assert_eq!(std::fs::read_dir(outside).unwrap().count(),0);
        close_fixture(_temp,&root);
    }
    #[tokio::test]
    async fn local_restart_cleanup_does_not_turn_an_unknown_operation_into_a_success_or_replay_it() {
        let (_temp,root,db)=setup();let manager=crate::LocalFileManager::open(&db).await.unwrap();
        let (guard,stage)=fixture(&db,&root,true);let id=guard.record.request_id;let parent_identity=guard.record.parent_identity.clone();
        let path=encode_path(&root.join("public"));let op=pab_protocol::FileManagerOperation::PrepareDirectory {path:path.clone(),expected_parent:parent_identity,existing_object:None};
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&db)).await.unwrap();
        sqlx::query("INSERT INTO local_manager_requests(id,owner,spec,created) VALUES(?,?,?,?)").bind(id.to_string()).bind("owner").bind(serde_json::to_string(&op).unwrap()).bind(crate::now_ms()).execute(&pool).await.unwrap();
        // Leave an inactive, known-owned record for the reopening path to clean.
        std::fs::write(stage.join("entry/sentinel"),b"temporary blocker").unwrap();drop(guard);std::fs::remove_file(stage.join("entry/sentinel")).unwrap();
        std::fs::create_dir(root.join("public")).unwrap();std::fs::write(root.join("public/sentinel"),b"public replacement").unwrap();
        let reopened=crate::LocalFileManager::open(&db).await.unwrap();assert!(!stage.exists());assert_eq!(std::fs::read_dir(db.with_extension("directory-staging")).unwrap().count(),0);
        assert!(reopened.observe("owner",id,&op).await.unwrap().is_none());let (_send,cancel)=tokio::sync::watch::channel(false);
        assert!(matches!(reopened.execute("owner",id,&op,cancel).await.unwrap().result,pab_protocol::FileManagerResult::Error {error} if error.code=="unconfirmed"));
        assert!(manager.observe("foreign",id,&op).await.is_err());assert_eq!(std::fs::read(root.join("public/sentinel")).unwrap(),b"public replacement");pool.close().await;
        close_fixture(_temp,&root);
    }
    #[test]
    fn active_records_are_locked_and_unexpected_child_content_is_retained() {
        let (_temp,root,db)=setup();let (guard,stage)=fixture(&db,&root,true);
        let report=recover_directory_staging(&db).unwrap();assert_eq!((report.cleaned,report.busy,report.retained),(0,1,0));assert!(stage.join("entry").is_dir());
        std::fs::write(stage.join("entry/sentinel"),b"do not delete").unwrap();drop(guard);
        let report=recover_directory_staging(&db).unwrap();assert_eq!((report.cleaned,report.busy,report.retained),(0,0,1));assert_eq!(std::fs::read(stage.join("entry/sentinel")).unwrap(),b"do not delete");
        std::fs::remove_file(stage.join("entry/sentinel")).unwrap();assert_eq!(recover_directory_staging(&db).unwrap().cleaned,1);assert!(!stage.exists());
        close_fixture(_temp,&root);
    }
    #[test]
    fn replaced_parent_container_or_child_is_never_adopted_for_cleanup() {
        for kind in ["parent","private","child"] {
            let (_temp,root,db)=setup();let parent=root.join("parent");std::fs::create_dir(&parent).unwrap();let (guard,stage)=fixture(&db,&parent,true);
            let path=match kind {"parent"=>parent.clone(),"private"=>stage.clone(),_=>stage.join("entry")};let moved=path.with_file_name(format!("moved-{kind}"));
            std::fs::rename(&path,&moved).unwrap();std::fs::create_dir(&path).unwrap();std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o700)).unwrap();std::fs::write(path.join("sentinel"),b"replacement").unwrap();
            drop(guard);let report=recover_directory_staging(&db).unwrap();assert_eq!((report.cleaned,report.retained),(0,1));assert_eq!(std::fs::read(path.join("sentinel")).unwrap(),b"replacement");assert!(moved.is_dir());
            std::fs::remove_file(path.join("sentinel")).unwrap();std::fs::remove_dir(&path).unwrap();std::fs::rename(&moved,&path).unwrap();assert_eq!(recover_directory_staging(&db).unwrap().cleaned,1);
            close_fixture(_temp,&root);
        }
    }
    #[test]
    fn foreign_journal_entries_links_and_fifos_are_retained_without_reading_or_following() {
        let (_temp,root,db)=setup();let _journal=Journal::open(&db,true).unwrap().unwrap();let journal_root=db.with_extension("directory-staging");
        let outside=root.join("outside");std::fs::write(&outside,b"outside").unwrap();
        std::fs::write(journal_root.join("foreign"),b"unowned").unwrap();
        symlink(&outside,journal_root.join(format!("{}.jsonl",RequestId::new()))).unwrap();
        std::fs::hard_link(&outside,journal_root.join(format!("{}.jsonl",RequestId::new()))).unwrap();
        assert!(Command::new("mkfifo").arg(journal_root.join(format!("{}.jsonl",RequestId::new()))).status().unwrap().success());
        let start=Instant::now();let report=recover_directory_staging(&db).unwrap();assert!(start.elapsed()<Duration::from_secs(2));assert_eq!((report.cleaned,report.busy,report.retained),(0,0,4));assert_eq!(std::fs::read(outside).unwrap(),b"outside");
        close_fixture(_temp,&root);
    }
    #[test]
    fn corrupt_complete_records_and_oversized_receipts_fail_closed() {
        for oversized in [false,true] {
            let (_temp,root,db)=setup();let (mut guard,stage)=fixture(&db,&root,true);
            if oversized {guard.file.write_all(&vec![b'x';MAX_RECORD_BYTES as usize+1]).unwrap();}else {let mut changed=guard.record.clone();changed.parent=encode_path(&root.join("foreign-parent"));guard.file.write_all(&serde_json::to_vec(&changed).unwrap()).unwrap();guard.file.write_all(b"\n").unwrap();}
            drop(guard);let report=recover_directory_staging(&db).unwrap();assert_eq!((report.cleaned,report.retained),(0,1));assert!(stage.join("entry").is_dir());
            close_fixture(_temp,&root);
        }
    }
    struct OwnedChild(Child);
    impl Drop for OwnedChild {fn drop(&mut self) {let _=self.0.kill();let _=self.0.wait();}}
    #[test]
    fn killed_creators_release_journal_locks_and_only_recorded_empty_staging_is_reclaimed() {
        for phase in ["intent","private","entry","published","private-unconfirmed","entry-unconfirmed","torn-tail"] {
            let (_temp,root,db)=setup();let mut child=OwnedChild(Command::new(std::env::current_exe().unwrap()).args(["--ignored","--exact","directory_staging::tests::crash_child","--nocapture"]).env("PAB_DIRECTORY_CRASH_ROOT",&root).env("PAB_DIRECTORY_CRASH_PHASE",phase).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
            let deadline=Instant::now()+Duration::from_secs(15);
            while !root.join("ready").exists() {assert!(child.0.try_wait().unwrap().is_none(),"child stopped early at {phase}");assert!(Instant::now()<deadline,"child never reached {phase}");std::thread::sleep(Duration::from_millis(20));}
            assert_eq!(recover_directory_staging(&db).unwrap().busy,1,"live {phase} must stay locked");
            child.0.kill().unwrap();let status=child.0.wait().unwrap();assert!(!status.success());
            let report=recover_directory_staging(&db).unwrap();let uncertain=phase.ends_with("unconfirmed");
            assert_eq!((report.cleaned,report.busy,report.retained),if uncertain {(0,0,1)}else{(1,0,0)},"phase {phase}");
            let stages=std::fs::read_dir(&root).unwrap().filter(|entry|entry.as_ref().unwrap().file_name().to_string_lossy().starts_with(".pab-dir-")).count();assert_eq!(stages,usize::from(uncertain),"phase {phase}");
            assert_eq!(root.join("public").exists(),phase=="published");
            if phase=="published" {assert_eq!(std::fs::read(root.join("public/sentinel")).unwrap(),b"published");}
            assert_eq!(recover_directory_staging(&db).unwrap().cleaned,0);
            let pid=child.0.id();drop(child);close_fixture(_temp,&root);
            eprintln!("directory-staging phase={phase} child_pid={pid} exit={status} retained={} fixture_cleanup_verified=true",report.retained);
        }
    }
    // Explicitly launched by the parent test; it must never run independently.
    #[test]
    #[ignore]
    fn crash_child() {
        let root=std::path::PathBuf::from(std::env::var_os("PAB_DIRECTORY_CRASH_ROOT").unwrap());let phase=std::env::var("PAB_DIRECTORY_CRASH_PHASE").unwrap();let db=root.join("index.sqlite3");
        let wait=||->! {std::fs::write(root.join("ready"),b"ready").unwrap();loop {std::thread::park_timeout(Duration::from_secs(30));}};
        let journal=Journal::open(&db,true).unwrap().unwrap();let parent=File::open(&root).unwrap();let identity=pab_os_control::filesystem::file_object_identity(&parent).unwrap();let mut guard=journal.begin(RequestId::new(),&encode_path(&root),&identity).unwrap();
        if phase=="intent" {wait();}
        let staging=guard.record.staging_name.clone();
        let object=pab_os_control::filesystem::create_directory_at_tracked(&parent,OsStr::new("public"),OsStr::new(&staging),|private,entry| {
            if phase=="private-unconfirmed" && entry.is_none() || phase=="entry-unconfirmed" && entry.is_some() {wait();}
            guard.checkpoint(private,entry)?;
            if phase=="private" && entry.is_none() || phase=="entry" && entry.is_some() {wait();}
            if phase=="torn-tail" && entry.is_some() {guard.file.write_all(b"{\"torn\"").unwrap();guard.file.sync_all().unwrap();wait();}
            Ok(())
        }).unwrap();
        assert!(object.metadata().unwrap().is_dir());std::fs::write(root.join("public/sentinel"),b"published").unwrap();wait();
    }
}
