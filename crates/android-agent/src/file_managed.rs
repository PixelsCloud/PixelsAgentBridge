//! Managed browser snapshots over the same SDK-granted mounts as MCP file IO.
use super::*;
#[derive(Clone)]
pub(super) struct Directory {
    owner:String,path:NativeFilePath,entries:Vec<ManagedFileEntry>,created:tokio::time::Instant,observed:i64,
}
fn invalid(message:impl Into<String>)->FileError {FileError::new("invalid_request","managed",message)}
fn path_text(path:&NativeFilePath)->Result<&str,FileError> {
    match path {NativeFilePath::Utf8(path) if path.starts_with('/')=>Ok(path),_=>Err(invalid("Android mounts require UTF-8 absolute virtual paths"))}
}
impl FileService {
    pub(super) async fn authorize_managed_reply(&self,reply:&FileManagerReply)->Result<(),FileError> {
        let paths:Vec<String>=match &reply.result {
            FileManagerResult::Locations {locations}=>std::iter::once("/".to_owned()).chain(locations.roots.iter().map(|r|r.path.display_label())).collect(),
            FileManagerResult::Opened {snapshot_id}=>vec![self.managed_directories.lock().await.get(snapshot_id).ok_or_else(||invalid("Directory snapshot expired"))?.path.display_label()],
            FileManagerResult::Directory {page}=>vec![page.path.display_label()],
            FileManagerResult::Entry {entry}=>vec![entry.path.display_label()],
            FileManagerResult::DirectoryObject {path,..}|FileManagerResult::PathIdentity {path,..}|FileManagerResult::DirectoryMetadata {path,..}|FileManagerResult::DirectoryPrepared {path,..}|FileManagerResult::Applied {path}=>vec![path.display_label()],
            _=>vec!["/".into()],
        };
        for path in paths {
            let value=tokio::time::timeout(Duration::from_secs(2),self.authorization.call(json!({"action":"authorize","path":path}))).await.map_err(|_|invalid("Storage authorization unavailable"))?.map_err(|_|invalid("Storage authorization unavailable"))?;
            if let Some(error)=value["error"].as_str(){return Err(FileError::new(error,"authorize","Current Android storage authorization denied"));}
        }
        Ok(())
    }
    pub(super) async fn managed(&self,owner:&str,request:&FileSystemRequest,operation:&FileManagerOperation,expected:Option<&FileExecutionIdentity>,reply:&mut FileSystemReply,cancel:&Arc<AtomicBool>)->Result<(),FileError> {
        let context=self.native_context.get().ok_or_else(||invalid("Android execution context unavailable"))?.clone();
        if expected.is_some_and(|value|!value.matches_context(&context)){return Err(FileError::new("execution_context_unavailable","managed","Android application identity changed"));}
        let scope=blake3::keyed_hash(&self.digest_key,b"android-managed-filesystem").to_hex().to_string();
        let result=match operation {
            FileManagerOperation::OpenDirectory {path}=>{
                let value=self.io(json!({"action":"managed_list","path":path_text(path)?})).await?;
                let entries:Vec<ManagedFileEntry>=serde_json::from_value(value["entries"].clone()).map_err(|_|invalid("Invalid Android directory reply"))?;
                if entries.len()>20000 || entries.iter().any(|e|e.validate().is_err()){return Err(invalid("Android directory exceeds limits"));}
                cancelled(cancel)?;
                let mut snapshots=self.managed_directories.lock().await;
                snapshots.retain(|_,v|v.created.elapsed()<Duration::from_secs(15*60));
                if snapshots.len()>=16 || snapshots.values().map(|v|v.entries.len()).sum::<usize>()+entries.len()>40000 {return Err(FileError::new("capacity","managed","Close another folder before opening this directory"));}
                let id=RequestId::new();snapshots.insert(id,Directory {owner:owner.into(),path:path.clone(),entries,created:tokio::time::Instant::now(),observed:crate::peer::now_ms()});
                FileManagerResult::Opened {snapshot_id:id}
            }
            FileManagerOperation::ReadDirectory {snapshot_id,after,limit,filter}=>{
                let snapshot=self.managed_directories.lock().await.get(snapshot_id).filter(|v|v.owner==owner && v.created.elapsed()<Duration::from_secs(15*60)).cloned().ok_or_else(||FileError::new("snapshot_unavailable","managed","Directory snapshot expired; refresh this folder"))?;
                self.io(json!({"action":"stat","path":path_text(&snapshot.path)?})).await?;
                let query=filter.query.to_lowercase();
                let mut entries:Vec<_>=snapshot.entries.iter().filter(|e|(filter.show_hidden||!e.hidden)&&e.name.to_lowercase().contains(&query)).cloned().collect();
                entries.sort_by(|a,b|{
                    let folders=matches!(b.kind,ManagedFileKind::Directory).cmp(&matches!(a.kind,ManagedFileKind::Directory));
                    let order=match filter.sort {FileManagerSort::Name=>a.name.to_lowercase().cmp(&b.name.to_lowercase()),FileManagerSort::Size=>a.size.as_ref().map(|v|v.0).cmp(&b.size.as_ref().map(|v|v.0)),FileManagerSort::Modified=>a.modified_at_unix_ms.cmp(&b.modified_at_unix_ms)}.then(a.name.cmp(&b.name));
                    folders.then(if filter.descending {order.reverse()} else {order})
                });
                let tag=digest(&serde_json::to_vec(filter).map_err(|_|invalid("Invalid filter"))?);
                let offset=match after {None=>0,Some(value)=>{let (prefix,n)=value.split_once(':').ok_or_else(||invalid("Invalid directory cursor"))?;if prefix!=tag {return Err(invalid("Directory cursor belongs to another filter"));}n.parse::<usize>().map_err(|_|invalid("Invalid directory offset"))?}};
                if offset>entries.len(){return Err(invalid("Directory offset exceeds snapshot"));}
                let total=entries.len();let mut page=Vec::new();let mut bytes=0;
                for entry in entries.into_iter().skip(offset).take(*limit as usize){let size=serde_json::to_vec(&entry).map_err(|_|invalid("Invalid directory entry"))?.len();if !page.is_empty()&&bytes+size>FILE_MANAGER_PAGE_BYTES-2048 {break;}bytes+=size;page.push(entry);}
                let next=offset+page.len();FileManagerResult::Directory {page:ManagedDirectoryPage {snapshot_id:*snapshot_id,path:snapshot.path.clone(),entries:page,next_after:(next<total).then(||format!("{tag}:{next}")),total_entries:FileByteCount(total as u64),observed_at_unix_ms:snapshot.observed,complete:true}}
            }
            FileManagerOperation::CloseDirectory {snapshot_id}=>{
                let mut snapshots=self.managed_directories.lock().await;
                if snapshots.get(snapshot_id).is_some_and(|v|v.owner!=owner){return Err(invalid("Directory belongs to another caller"));}
                snapshots.remove(snapshot_id);FileManagerResult::Closed {snapshot_id:*snapshot_id}
            }
            _=>{
                let value=self.io(json!({"action":"managed","path":"/","operation":operation,"filesystem_scope":scope})).await?;
                serde_json::from_value(value).map_err(|e|invalid(format!("Invalid Android managed result: {e}")))?
            }
        };
        let body=FileManagerReply {schema_version:FILE_MANAGER_SCHEMA_VERSION,request_id:request.request_id,execution_context:Some(context.clone()),result};
        body.validate_for(&FileManagerRequest {schema_version:FILE_MANAGER_SCHEMA_VERSION,request_id:request.request_id,execution:request.execution,expected_execution:expected.cloned(),operation:operation.clone()}).map_err(invalid)?;
        reply.execution_context=Some(context);reply.file_manager=Some(body);Ok(())
    }
}
