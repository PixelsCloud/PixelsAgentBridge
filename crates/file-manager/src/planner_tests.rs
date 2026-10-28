use crate::*;
use pab_protocol::*;
use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
use tokio::sync::watch;

struct PauseBrowser {inner:LocalFileBrowser,store:BatchStore,batch:RequestId,reads:Arc<AtomicUsize>}
impl FileBrowser for PauseBrowser {
    fn execute<'a>(&'a self,operation:FileManagerOperation,cancel:watch::Receiver<bool>)->std::pin::Pin<Box<dyn std::future::Future<Output=Result<FileManagerReply,FileManagerError>>+Send+'a>> {
        Box::pin(async move {
            let page=matches!(operation,FileManagerOperation::ReadDirectory {..});
            let reply=self.inner.execute(operation,cancel).await?;
            if page && self.reads.fetch_add(1,Ordering::SeqCst)==1 {self.store.set_intent("owner",self.batch,FileTransferIntent::Pause).await?;}
            Ok(reply)
        })
    }
}
#[tokio::test]
async fn tree_scan_recovers_a_partially_committed_page_without_duplicates_and_keeps_dependencies() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
    let source=root.join("source");let destination=root.join("destination");
    std::fs::create_dir_all(source.join("empty")).unwrap();std::fs::create_dir_all(source.join("nested")).unwrap();std::fs::create_dir(&destination).unwrap();
    for n in 0..300 {std::fs::write(source.join(format!("file-{n:04}")),b"a").unwrap();}
    std::fs::write(source.join("nested/child"),b"abc").unwrap();
    let store=BatchStore::open(&root.join("batch.sqlite3")).await.unwrap();
    let local=LocalFileManager::open(&root.join("local.sqlite3")).await.unwrap();
    let style=if cfg!(windows){PathStyle::Windows}else{PathStyle::Posix};
    let spec=FileBatchSpec {id:RequestId::new(),device:DeviceRef {tenant_id:TenantId::new(),device_id:DeviceId::new()},code:"123456789".parse().unwrap(),direction:FileTransferDirection::Upload,sources:vec![encode_path(&source.join("nested/child")),encode_path(&source),encode_path(&source.join("nested")),encode_path(&source)],destination:encode_path(&destination),execution:ExecutionSelection::Service {},conflict:FileConflictPolicy::Ask,force_relay:false};
    store.create("owner",&spec).await.unwrap();
    let browser=PauseBrowser {inner:local.browser("batch-source".into()),store:store.clone(),batch:spec.id,reads:Arc::new(AtomicUsize::new(0))};
    let (_send,cancel)=watch::channel(false);
    assert!(plan_tree(&store,"owner",&spec,&browser,style,style,cancel.clone()).await.is_err());
    store.acknowledge_scan_stopped("owner",spec.id).await.unwrap();
    let paused=store.snapshot("owner",spec.id).await.unwrap();
    assert_eq!(paused.phase,"paused");assert!(paused.total.0>1 && paused.total.0<304,"one bounded directory page must have committed: {}",paused.total.0);
    let restarted=BatchStore::open(&root.join("batch.sqlite3")).await.unwrap();
    restarted.set_intent("owner",spec.id,FileTransferIntent::Run).await.unwrap();
    plan_tree(&restarted,"owner",&spec,&local.browser("batch-restarted".into()),style,style,cancel).await.unwrap();
    let planned=restarted.snapshot("owner",spec.id).await.unwrap();
    assert_eq!(planned.total.0,304);assert_eq!(planned.size.0,303);assert!(planned.scan_complete);
    let first=restarted.items("owner",spec.id,0,256).await.unwrap();let second=restarted.items("owner",spec.id,256,256).await.unwrap();
    let unique=first.iter().chain(&second).map(|item|item.spec.id).collect::<std::collections::HashSet<_>>();assert_eq!(unique.len(),304);
    assert!(first.iter().chain(&second).any(|item|item.spec.kind==ManagedFileKind::Directory && item.spec.relative_name.ends_with("/empty")));
    let worker=RequestId::new();restarted.heartbeat("owner",worker).await.unwrap();
    let parent=restarted.claim("owner",spec.id,worker).await.unwrap().unwrap();assert_eq!(parent.spec.kind,ManagedFileKind::Directory);
    assert!(restarted.claim("owner",spec.id,worker).await.unwrap().is_none(),"children must wait for their destination directory");
    assert!(restarted.list("foreign",10).await.unwrap().is_empty());
    local.release("batch-source").await.unwrap();local.release("batch-restarted").await.unwrap();
}
