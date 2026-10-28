//! Bounded, restartable tree enumeration. Directory snapshots may expire; the
//! durable queue and deterministic item IDs allow a rescan without duplicates.
use crate::{BatchStore,FileManagerError,native_join,native_name};
use pab_protocol::{FileBatchItemSpec,FileBatchSpec,FileByteCount,FileManagerFilter,FileManagerOperation as Op,FileManagerReply,FileManagerResult as Body,FileTransferIntent,ManagedFileEntry,ManagedFileKind,NativeFilePath,PathStyle,RequestId};
use std::{future::Future,pin::Pin};
use tokio::sync::watch;

pub trait FileBrowser: Send+Sync {
    fn execute<'a>(&'a self,operation:Op,cancel:watch::Receiver<bool>)->Pin<Box<dyn Future<Output=Result<FileManagerReply,FileManagerError>>+Send+'a>>;
}
async fn execute(browser:&impl FileBrowser,operation:Op,cancel:&watch::Receiver<bool>)->Result<Body,FileManagerError> {
    let reply = browser.execute(operation,cancel.clone()).await?;
    match reply.result {Body::Error {error}=>Err(FileManagerError::Io(std::io::Error::other(format!("{}: {}",error.code,error.message)))),body=>Ok(body)}
}
fn stable_item(batch:RequestId,path:&NativeFilePath)->Result<RequestId,FileManagerError> {
    let data = serde_json::to_vec(&(batch,path))?;
    let hash = blake3::hash(&data);
    Ok(RequestId::from_u128(u128::from_le_bytes(hash.as_bytes()[..16].try_into().unwrap())))
}
pub async fn plan_tree(store:&BatchStore,owner:&str,spec:&FileBatchSpec,source:&impl FileBrowser,source_style:PathStyle,destination_style:PathStyle,cancel:watch::Receiver<bool>)->Result<(),FileManagerError> {
    // Repeating this prefix after pause/restart only verifies immutable specs;
    // it cannot append the same file a second time or silently change it.
    let mut roots = Vec::new();
    for path in &crate::native_paths::independent_sources(&spec.sources,source_style)? {
        ensure_running(store,owner,spec.id,&cancel).await?;
        let Body::Entry {entry} = execute(source,Op::Inspect {path:path.clone()},&cancel).await? else {return Err(FileManagerError::Invalid("missing source metadata"));};
        let name = native_name(path,source_style)?;
        let destination = native_join(&spec.destination,&portable_name(&name,destination_style)?,destination_style)?;
        roots.push(planned_entry(spec.id,entry,destination,None,name.display_label())?);
        if roots.len()==256 {store.append_planned_batch(owner,spec.id,&roots).await?;roots.clear();}
    }
    if !roots.is_empty(){store.append_planned_batch(owner,spec.id,&roots).await?;}
    while let Some(directory) = store.next_scan_directory(owner,spec.id).await? {
        ensure_running(store,owner,spec.id,&cancel).await?;
        let Body::Opened {snapshot_id} = execute(source,Op::OpenDirectory {path:directory.source.clone()},&cancel).await? else {return Err(FileManagerError::Invalid("missing source directory snapshot"));};
        let result = async {
            let mut after = None;
            loop {
                ensure_running(store,owner,spec.id,&cancel).await?;
                let Body::Directory {page} = execute(source,Op::ReadDirectory {snapshot_id,after:after.clone(),limit:256,filter:FileManagerFilter {show_hidden:true,..Default::default()}},&cancel).await? else {return Err(FileManagerError::Invalid("missing source directory page"));};
                let mut items = Vec::with_capacity(page.entries.len());
                for entry in page.entries {
                    let name = native_name(&entry.path,source_style)?;
                    let destination = native_join(&directory.destination,&portable_name(&name,destination_style)?,destination_style)?;
                    let label = format!("{}/{}",directory.relative_name,entry.name);
                    items.push(planned_entry(spec.id,entry,destination,Some(directory.item),label)?);
                }
                store.append_planned_batch(owner,spec.id,&items).await?;
                let Some(next) = page.next_after else {break};
                if after.as_ref() == Some(&next) {return Err(FileManagerError::Invalid("directory cursor did not advance"));}
                after = Some(next);
            }
            store.complete_scan_directory(owner,spec.id,directory.item).await
        }.await;
        // Snapshot lifetime is independent of the batch lifetime. Cleanup uses
        // a fresh non-cancelled token even when pause/cancel won the scan race.
        let (_send,cleanup) = watch::channel(false);
        let _ = execute(source,Op::CloseDirectory {snapshot_id},&cleanup).await;
        result?;
    }
    ensure_running(store,owner,spec.id,&cancel).await?;
    store.finish_scan(owner,spec.id,None).await
}
async fn ensure_running(store:&BatchStore,owner:&str,batch:RequestId,cancel:&watch::Receiver<bool>)->Result<(),FileManagerError> {
    if *cancel.borrow() || store.scan_intent(owner,batch).await? != FileTransferIntent::Run {Err(FileManagerError::Interrupted)}else{Ok(())}
}
fn planned_entry(batch:RequestId,entry:ManagedFileEntry,destination:NativeFilePath,parent:Option<RequestId>,label:String)->Result<crate::PlannedItem,FileManagerError> {
    let spec = FileBatchItemSpec {id:stable_item(batch,&entry.path)?,source:entry.path,destination,relative_name:label.chars().take(4096).collect(),kind:entry.kind,size:if entry.kind == ManagedFileKind::File {entry.size.ok_or(FileManagerError::Invalid("file size unavailable"))?}else{FileByteCount(0)},modified_at_unix_ms:entry.modified_at_unix_ms,unix_mode:entry.unix_mode};
    let skip = match entry.kind {ManagedFileKind::Symlink=>Some("Symbolic links are not followed or transferred"),ManagedFileKind::Other=>Some("Unsupported filesystem object"),_=>None};
    if entry.error.is_some() {return Err(FileManagerError::Io(std::io::Error::other(entry.error.unwrap())));}
    Ok(crate::PlannedItem {spec,parent,skip:skip.map(str::to_owned)})
}
/// Cross-platform conversion is allowed only when the complete component is
/// valid Unicode. Unrepresentable raw bytes/surrogates never become U+FFFD.
pub fn portable_name(name:&NativeFilePath,target:PathStyle)->Result<NativeFilePath,FileManagerError> {
    match (name,target) {
        (NativeFilePath::UnixBytes(bytes),PathStyle::Windows)=>String::from_utf8(bytes.clone()).map(NativeFilePath::Utf8).map_err(|_|FileManagerError::Invalid("filename cannot be represented on Windows")),
        (NativeFilePath::WindowsWide(units),PathStyle::Posix)=>String::from_utf16(units).map(NativeFilePath::Utf8).map_err(|_|FileManagerError::Invalid("filename cannot be represented on POSIX")),
        _=>Ok(name.clone()),
    }
}
