//! Whole SDK -> QUIC -> actual Executor filesystem acceptance. Synthetic
//! account/actor fixtures only; no installed application, desktop or real token.
use super::*;
use pab_file_manager::{BatchStore,LocalFileManager,encode_path};
use pab_protocol::*;
use std::path::Path;

fn fixture_directory(prefix:&str)->tempfile::TempDir {
    let mut builder=tempfile::Builder::new();builder.prefix(prefix);
    let temp=match std::env::var_os("PAB_FILE_TRANSFER_FIXTURE_ROOT") {
        Some(parent)=>{
            let parent=std::fs::canonicalize(parent).expect("explicit fixture parent must exist");
            builder.tempdir_in(parent).expect("create isolated fixture under explicit parent")
        },
        None=>builder.tempdir().unwrap(),
    };
    eprintln!("PAB_FILE_TRANSFER_FIXTURE_ROOT={}",temp.path().canonicalize().unwrap().display());
    temp
}

#[tokio::test]
async fn sdk_binds_upload_remote_parent_and_download_local_parent_before_staging() {
    Box::pin(public_parent_case()).await;
}
async fn public_parent_case() {
    use crate::connection::TransferControl;
    let temp=fixture_directory("pab-public-parent-");let root=temp.path().canonicalize().unwrap();
    let remote=root.join("remote");let local=root.join("local");std::fs::create_dir(&remote).unwrap();std::fs::create_dir(&local).unwrap();
    let constraint=|path:&Path|FileTransferParentConstraint {identity:pab_file_manager::DirectoryObject::open(&encode_path(path),false).unwrap().identity().to_owned()};
    let remote_parent=constraint(&remote);let local_parent=constraint(&local);assert_ne!(remote_parent,local_parent);
    let source=root.join("source.bin");let destination=remote.join("result.bin");let received=local.join("received.bin");let bytes=vec![19u8;131073];std::fs::write(&source,&bytes).unwrap();
    std::fs::rename(&remote,root.join("original-remote")).unwrap();std::fs::create_dir(&remote).unwrap();std::fs::write(&destination,b"preserve replacement").unwrap();
    let (first,second,client,server)=crate::connection::test_connection_pair().await;
    let actor=client.operator();let context=pab_platform::detect_native_execution_context().unwrap();
    let service=pab_executor::test_support::ProtocolService::open(&root.join("executor.sqlite3"),client.device_ref(),context).await.unwrap();
    let (stop,mut stopped)=watch::channel(false);
    let receiver=tokio::spawn(async move {
        let mut tasks=tokio::task::JoinSet::new();let mut failures=Vec::new();
        loop {tokio::select! {
            _=stopped.changed()=>break,
            stream=server.accept_bi(Duration::from_secs(30))=>{let Ok(stream)=stream else {break};let service=service.clone();tasks.spawn(async move {Box::pin(service.handle_stream(actor,stream,Duration::from_secs(10))).await});},
            Some(result)=tasks.join_next(),if !tasks.is_empty()=>{if let Err(error)=result.unwrap(){failures.push(error);}},
        }}
        while let Some(result)=tasks.join_next().await {if let Err(error)=result.unwrap(){failures.push(error);}}failures
    });
    let options=FileTransferOptions {destination_parent:Some(remote_parent),..Default::default()};
    assert!(client.upload_native_file_with_options(RequestId::new(),&source,&encode_path(&destination),true,&options,&TransferControl::default(),|_,_|{}).await.is_err());
    assert_eq!(std::fs::read(&destination).unwrap(),b"preserve replacement");assert_eq!(std::fs::read_dir(&remote).unwrap().count(),1);
    std::fs::rename(&remote,root.join("replacement-remote")).unwrap();std::fs::rename(root.join("original-remote"),&remote).unwrap();
    client.upload_native_file_with_options(RequestId::new(),&source,&encode_path(&destination),true,&options,&TransferControl::default(),|_,_|{}).await.unwrap();assert_eq!(std::fs::read(&destination).unwrap(),bytes);
    let options=FileTransferOptions {destination_parent:Some(local_parent),..Default::default()};
    std::fs::rename(&local,root.join("original-local")).unwrap();std::fs::create_dir(&local).unwrap();std::fs::write(&received,b"preserve local replacement").unwrap();
    let rejected=TransferControl::default();
    assert!(client.download_native_file_with_options(RequestId::new(),&encode_path(&destination),&received,true,&options,None,&rejected,|_,_|{}).await.is_err());assert!(!rejected.remote_requested());
    assert_eq!(std::fs::read(&received).unwrap(),b"preserve local replacement");assert_eq!(std::fs::read_dir(&local).unwrap().count(),1);
    std::fs::rename(&local,root.join("replacement-local")).unwrap();std::fs::rename(root.join("original-local"),&local).unwrap();
    client.download_native_file_with_options(RequestId::new(),&encode_path(&destination),&received,true,&options,None,&TransferControl::default(),|_,_|{}).await.unwrap();assert_eq!(std::fs::read(&received).unwrap(),bytes);
    assert_eq!(std::fs::read(root.join("replacement-remote/result.bin")).unwrap(),b"preserve replacement");assert_eq!(std::fs::read(root.join("replacement-local/received.bin")).unwrap(),b"preserve local replacement");
    stop.send_replace(true);let failures=receiver.await.unwrap();assert_eq!(failures.len(),1,"{failures:?}");assert!(failures[0].contains("parent changed"),"{failures:?}");
    first.close().await;second.close().await;
}

#[tokio::test]
async fn sdk_transfers_attributes_through_the_public_executor_and_rejects_stale_sources() {
    Box::pin(public_attributes_case()).await;
}
async fn public_attributes_case() {
    use crate::connection::TransferControl;
    let temp=fixture_directory("pab-public-attributes-");let root=temp.path().canonicalize().unwrap();
    let source=root.join("source.bin");let destination=root.join("remote.bin");let received=root.join("received.bin");
    let bytes=vec![71u8;131073];std::fs::write(&source,&bytes).unwrap();std::fs::write(&destination,b"original").unwrap();
    let metadata=FileTransferMetadata {modified_at_unix_ms:Some(1_700_000_000_123),unix_mode:Some(0o640)};
    let source_file=pab_file_manager::open_source(&source).unwrap();
    let observed=source_file.metadata().unwrap();drop(source_file);
    let constraint=FileTransferSourceConstraint {size:FileByteCount(bytes.len() as u64),modified_at_unix_ms:pab_file_manager::modified_at_unix_ms(&observed)};
    let (first,second,client,server)=crate::connection::test_connection_pair().await;
    let actor=client.operator();let context=pab_platform::detect_native_execution_context().unwrap();
    let options=FileTransferOptions { destination_parent:None,metadata:Some(metadata.clone()),source_constraint:Some(constraint.clone()),expected_execution:Some(FileExecutionIdentity::from_context(&context).unwrap()),..Default::default()};
    let service=pab_executor::test_support::ProtocolService::open(&root.join("executor.sqlite3"),client.device_ref(),context).await.unwrap();
    let (stop,mut stopped)=watch::channel(false);
    let receiver=tokio::spawn(async move {
        let mut tasks=tokio::task::JoinSet::new();let mut failures=Vec::new();
        loop {tokio::select! {
            _=stopped.changed()=>break,
            stream=server.accept_bi(Duration::from_secs(30))=>{let Ok(stream)=stream else {break};let service=service.clone();tasks.spawn(async move {Box::pin(service.handle_stream(actor,stream,Duration::from_secs(10))).await});},
            Some(result)=tasks.join_next(),if !tasks.is_empty()=>{if let Err(error)=result.unwrap(){failures.push(error);}},
        }}
        while let Some(result)=tasks.join_next().await {if let Err(error)=result.unwrap(){failures.push(error);}}failures
    });
    // A stale local selection is rejected before any upload is accepted.
    let mut stale=options.clone();stale.source_constraint.as_mut().unwrap().size.0+=1;
    let control=TransferControl::default();
    assert!(client.upload_native_file_with_options(RequestId::new(),&source,&encode_path(&destination),true,&stale,&control,|_,_|{}).await.is_err());
    assert!(!control.remote_requested());assert_eq!(std::fs::read(&destination).unwrap(),b"original");
    let upload=TransferControl::default();let upload_id=RequestId::new();
    client.upload_native_file_with_options(upload_id,&source,&encode_path(&destination),true,&options,&upload,|_,_|{}).await.unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(),bytes);
    let receipt=upload.accepted().unwrap();assert_eq!(receipt.request_id,upload_id);assert_eq!(receipt.state,"completed");assert_eq!(receipt.published,Some(true));
    let attributes=receipt.metadata_result.as_ref().unwrap();attributes.validate_for(&metadata).unwrap();assert_eq!(attributes.actual.modified_at_unix_ms,metadata.modified_at_unix_ms);
    #[cfg(windows)] {assert_eq!(attributes.warnings.len(),1);assert_eq!(attributes.warnings[0].code,"unsupported");}
    #[cfg(unix)] {assert_eq!(attributes.actual.unix_mode,Some(0o640));assert!(attributes.warnings.is_empty());}
    let mut download_options=options.clone();download_options.source_constraint.as_mut().unwrap().modified_at_unix_ms=metadata.modified_at_unix_ms;
    let download=TransferControl::default();
    client.download_native_file_with_options(RequestId::new(),&encode_path(&destination),&received,false,&download_options,None,&download,|_,_|{}).await.unwrap();
    assert_eq!(std::fs::read(&received).unwrap(),bytes);assert_eq!(download.local_publication(),Some(true));
    let result=download.local_metadata_result().unwrap();result.validate_for(&metadata).unwrap();assert_eq!(result.actual.modified_at_unix_ms,metadata.modified_at_unix_ms);
    // The receiver must preserve an existing destination when the remote
    // source no longer matches the selection, even if its size is unchanged.
    download_options.source_constraint.as_mut().unwrap().modified_at_unix_ms=Some(1);
    std::fs::write(&received,b"preserve existing destination").unwrap();
    let rejected=TransferControl::default();
    assert!(client.download_native_file_with_options(RequestId::new(),&encode_path(&destination),&received,true,&download_options,None,&rejected,|_,_|{}).await.is_err());
    assert_eq!(rejected.local_publication(),None);assert_eq!(std::fs::read(&received).unwrap(),b"preserve existing destination");
    stop.send_replace(true);let failures=receiver.await.unwrap();assert_eq!(failures.len(),1,"{failures:?}");assert!(failures[0].contains("source") || failures[0].contains("modified"),"{failures:?}");
    first.close().await;second.close().await;
}

fn assert_expected_stop_streams(failures:Vec<String>) {
    // Dropping a transfer stream during an acknowledged stop can make its
    // final response unwritable. Keep the accept loop alive so the separate
    // stop/query streams can finish, then validate every captured error.
    for failure in failures {
        assert_eq!(failure,"PAB stream failed: sending stopped by peer: error 0","unexpected stop fixture stream failure");
    }
}

async fn settled(job:&Arc<ManagedBatchJob>,wanted:&[&str])->pab_file_manager::BatchSnapshot {
    let result=tokio::time::timeout(Duration::from_secs(30),async {
        loop {let snapshot=job.snapshot().await.unwrap();if wanted.contains(&snapshot.phase.as_str()){return snapshot;}tokio::time::sleep(Duration::from_millis(50)).await;}
    }).await;
    match result {Ok(snapshot)=>snapshot,Err(_)=>panic!("batch {} did not settle: running={}, snapshot={:?}, items={:?}",job.spec.id,job.running(),job.snapshot().await,job.items(0).await)}
}
fn spec(device:DeviceRef,direction:FileTransferDirection,source:&Path,destination:&Path,conflict:FileConflictPolicy)->FileBatchSpec {
    FileBatchSpec {id:RequestId::new(),device,code:"123456789".parse().unwrap(),direction,sources:vec![encode_path(source)],destination:encode_path(destination),execution:ExecutionSelection::Service {},conflict,force_relay:false}
}

#[tokio::test]
async fn managed_batches_transfer_trees_empty_directories_and_resolve_real_conflicts_in_both_directions() {
    Box::pin(tree_case()).await;
}
async fn tree_case() {
    let temp=fixture_directory("pab-batch-tree-");let root=temp.path().canonicalize().unwrap();
    let source=root.join("source");let remote_root=root.join("remote");let local_root=root.join("received");
    std::fs::create_dir_all(source.join("nested/empty")).unwrap();std::fs::create_dir(&remote_root).unwrap();std::fs::create_dir(&local_root).unwrap();
    std::fs::write(source.join("first.bin"),(0..2_097_153).map(|n|(n%251)as u8).collect::<Vec<_>>()).unwrap();
    std::fs::write(source.join("zero"),[]).unwrap();std::fs::write(source.join("nested/中文 file.txt"),b"hello").unwrap();
    let (mut runtime,_credentials)=test_support::runtime(&root).await;
    let origin="https://file-batch-fixture.invalid";
    Arc::get_mut(&mut Arc::get_mut(&mut runtime).unwrap().inner).unwrap().store.account_origin=Some(origin.into());
    sqlx::query("INSERT INTO runtime_session_servers(session_id,origin) VALUES('fixture',?)").bind(origin).execute(&runtime.inner.store.pool).await.unwrap();
    let owner=runtime.file_operation_owner().await.unwrap();
    let (first,second,client,server)=crate::connection::test_connection_pair().await;
    let reference=client.device_ref();let actor=client.operator();
    runtime.inner.device_codes.lock().await.insert(reference,"123456789".parse().unwrap());
    runtime.inner.file_device(reference,false).await.unwrap().set_test_connection(client).await;
    let service=pab_executor::test_support::ProtocolService::open(&root.join("executor.sqlite3"),reference,pab_platform::detect_native_execution_context().unwrap()).await.unwrap();
    let (stop,mut stopped)=watch::channel(false);
    let receiver=tokio::spawn(async move {
        let mut tasks=tokio::task::JoinSet::new();
        let mut failures=Vec::new();
        loop {tokio::select! {
            _=stopped.changed()=>break,
            stream=server.accept_bi(Duration::from_secs(30))=>{
                let Ok(stream)=stream else {break};let service=service.clone();
                tasks.spawn(async move {Box::pin(service.handle_stream(actor,stream,Duration::from_secs(10))).await});
            },
            Some(result)=tasks.join_next(),if !tasks.is_empty()=>{if let Err(error)=result.unwrap(){failures.push(error);}},
        }}
        while let Some(result)=tasks.join_next().await {if let Err(error)=result.unwrap(){failures.push(error);}}
        failures
    });
    let store=BatchStore::open(&root.join("batches.sqlite3")).await.unwrap();
    let local=Arc::new(LocalFileManager::open(&root.join("local.sqlite3")).await.unwrap());
    let browser=RemoteFileBrowser::owned(runtime.clone(),reference,ExecutionSelection::Service {},false,owner.clone());
    let observation_path=remote_root.join("observation-only");
    let original=RequestId::new();let operation=FileManagerOperation::CreateDirectory {parent:encode_path(&remote_root),name:"observation-only".into()};
    assert!(browser.observe_request(original,operation.clone()).await.is_err());assert!(!observation_path.exists());
    let (_keep,cancel)=watch::channel(false);let applied=browser.execute_request(original,operation.clone(),cancel).await.unwrap();assert!(matches!(applied.result,FileManagerResult::Applied {..}));
    let rejected_id=RequestId::new();let stale=FileManagerOperation::SetDirectoryMetadata {path:encode_path(&observation_path),expected_object:"wrong:object".into(),metadata:FileTransferMetadata {modified_at_unix_ms:Some(123),unix_mode:None}};
    let (_keep,cancel)=watch::channel(false);let rejected=browser.execute_request(rejected_id,stale.clone(),cancel).await.unwrap();assert!(matches!(rejected.result,FileManagerResult::Error {..}));
    assert_eq!(browser.observe_request(rejected_id,stale).await.unwrap(),Some(rejected),"a known failed directory operation must not become permanently unconfirmed after its reply is lost");
    std::fs::remove_dir(&observation_path).unwrap();
    assert_eq!(browser.observe_request(original,operation).await.unwrap(),Some(applied));
    assert!(!observation_path.exists(),"observing an original result must not perform its mutation again");drop(browser);
    // Lose the batch receipt after an actual native create/merge. Replace the
    // target name before reopening the coordinator: recovery must use the
    // original journal outcome, never adopt or mutate the replacement object.
    for direction in [FileTransferDirection::Upload,FileTransferDirection::Download] {
        for merge in [false,true] {
            let case=root.join(format!("prepare-{}-{merge}",if direction==FileTransferDirection::Upload {"upload"}else{"download"}));
            let selected=case.join("selected");let destination=case.join("destination");std::fs::create_dir_all(&selected).unwrap();std::fs::create_dir(&destination).unwrap();
            let batch=spec(reference,direction,&selected,&destination,FileConflictPolicy::Ask);
            let (job,_)=ManagedBatchJob::prepare(runtime.clone(),store.clone(),local.clone(),batch.clone(),owner.clone()).await.unwrap();
            let target=destination.join("selected");if merge {std::fs::create_dir(&target).unwrap();}
            let existing=merge.then(||pab_file_manager::DirectoryObject::open(&encode_path(&target),false).unwrap().identity().to_owned());
            let entry=FileBatchItemSpec {id:RequestId::new(),source:encode_path(&selected),destination:encode_path(&target),relative_name:"selected".into(),kind:ManagedFileKind::Directory,size:FileByteCount(0),modified_at_unix_ms:None,unix_mode:None};
            store.append(&owner,batch.id,&[entry.clone()]).await.unwrap();store.finish_scan(&owner,batch.id,None).await.unwrap();
            let worker=RequestId::new();store.heartbeat(&owner,worker).await.unwrap();let claimed=store.claim(&owner,batch.id,worker).await.unwrap().unwrap();
            store.save_item_target(&owner,entry.id,worker,&pab_file_manager::ItemTarget {path:entry.destination.clone(),overwrite:false,choice:FileConflictPolicy::Ask}).await.unwrap();
            let prep=store.begin_directory_preparation(&owner,entry.id,worker,&entry.destination,if cfg!(windows){PathStyle::Windows}else{PathStyle::Posix},existing).await.unwrap().unwrap();
            let (_keep,cancel)=watch::channel(false);
            let receipt=if direction==FileTransferDirection::Upload {
                RemoteFileBrowser::owned(runtime.clone(),reference,ExecutionSelection::Service {},false,owner.clone()).execute_request(prep.request_id,prep.operation.clone(),cancel).await.unwrap()
            } else {local.browser(format!("batch:{}",batch.id)).execute_request(prep.request_id,prep.operation.clone(),cancel).await.unwrap()};
            let FileManagerResult::DirectoryPrepared {identity,..}=&receipt.result else {panic!("{:?}",receipt.result)};
            if direction==FileTransferDirection::Download {
                let browser=local.browser(format!("batch:{}",batch.id));assert_eq!(browser.retained_receipts(None).await.unwrap().len(),1);
                let journal=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(root.join("local.sqlite3"))).await.unwrap();
                sqlx::query("UPDATE local_manager_requests SET created=created-172800000 WHERE id=?").bind(prep.request_id.to_string()).execute(&journal).await.unwrap();journal.close().await;
                let (_send,cancel)=watch::channel(false);
                local.execute("ttl-probe",RequestId::new(),&FileManagerOperation::CreateDirectory {parent:encode_path(&case),name:NativeFilePath::Utf8("ttl-collection".into())},cancel).await.unwrap();
                assert_eq!(browser.observe_request(prep.request_id,&prep.operation).await.unwrap(),Some(receipt.clone()),"the real batch consumer must retain its original receipt across TTL collection");
            }
            assert!(store.directory_preparation(&owner,prep.request_id).await.unwrap().unwrap().result.is_none());
            let moved=destination.join("original-moved");std::fs::rename(&target,&moved).unwrap();std::fs::create_dir(&target).unwrap();std::fs::write(target.join("sentinel"),b"replacement").unwrap();
            let replacement=pab_file_manager::DirectoryObject::open(&encode_path(&target),false).unwrap().identity().to_owned();assert_ne!(&replacement,identity);
            store.retire_worker(&owner,batch.id,worker).await.unwrap();drop(job);
            let reopened=BatchStore::open(&root.join("batches.sqlite3")).await.unwrap();let (recovered,inserted)=ManagedBatchJob::prepare(runtime.clone(),reopened.clone(),local.clone(),batch,owner.clone()).await.unwrap();assert!(!inserted);
            recovered.observe_once_for_test(entry.id).await.unwrap();
            let ready=reopened.directory_ready(&owner,entry.id).await.unwrap().unwrap();assert_eq!(&ready.identity,identity);assert_eq!(ready.path,entry.destination);
            assert_eq!(reopened.item(&owner,entry.id).await.unwrap().attempt,claimed.attempt);
            assert_eq!(reopened.directory_preparation(&owner,prep.request_id).await.unwrap().unwrap().result,Some(receipt.result));
            if direction==FileTransferDirection::Download {assert!(local.browser(format!("batch:{}",recovered.spec.id)).retained_receipts(None).await.unwrap().is_empty(),"recorded original preparation must release its receipt retention");}
            assert_eq!(std::fs::read(target.join("sentinel")).unwrap(),b"replacement");assert_eq!(std::fs::read_dir(&target).unwrap().count(),1);
            assert!(pab_file_manager::DirectoryObject::prepare(&encode_path(&target),pab_file_manager::DirectoryObject::open(&encode_path(&destination),false).unwrap().identity(),Some(&ready.identity)).is_err(),"a later step must reject the replacement even after original-result recovery");
        }
    }
    let upload_spec=spec(reference,FileTransferDirection::Upload,&source,&remote_root,FileConflictPolicy::Ask);
    assert!(store.reserve_submission(&owner,&upload_spec).await.unwrap());
    assert!(!store.reserve_submission(&owner,&upload_spec).await.unwrap());
    let (upload,inserted)=ManagedBatchJob::prepare(runtime.clone(),store.clone(),local.clone(),upload_spec,owner.clone()).await.unwrap();assert!(!inserted);upload.launch();
    let done=settled(&upload,&["completed","failed","partial_failed"]).await;
    assert_eq!(done.phase,"completed","{:?}",done.message);assert_eq!(done.completed.0,6);
    let remote=remote_root.join("source");
    assert!(remote.join("nested/empty").is_dir());assert_eq!(std::fs::read(remote.join("first.bin")).unwrap(),std::fs::read(source.join("first.bin")).unwrap());assert_eq!(std::fs::metadata(remote.join("zero")).unwrap().len(),0);
    for item in upload.items(0).await.unwrap().into_iter().filter(|item|matches!(item.spec.kind,ManagedFileKind::File|ManagedFileKind::Directory)) {
        let attributes=item.metadata_result.unwrap();assert_eq!(attributes.actual.modified_at_unix_ms,item.spec.modified_at_unix_ms);
        let path=pab_file_manager::decode_path(&item.spec.destination).unwrap();assert_eq!(pab_file_manager::modified_at_unix_ms(&std::fs::metadata(path).unwrap()),item.spec.modified_at_unix_ms);
    }
    std::fs::write(source.join("first.bin"),b"new").unwrap();
    let (conflicts,_)=ManagedBatchJob::prepare(runtime.clone(),store.clone(),local.clone(),spec(reference,FileTransferDirection::Upload,&source,&remote_root,FileConflictPolicy::Ask),owner.clone()).await.unwrap();conflicts.launch();
    settled(&conflicts,&["waiting_conflict"]).await;
    let original=std::fs::read(remote.join("first.bin")).unwrap();assert!(original.len()>2_000_000);
    // Wait until both workers have made all currently ready conflicts visible.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let conflict_deadline=tokio::time::Instant::now()+Duration::from_secs(30);
    loop {
        assert!(tokio::time::Instant::now()<conflict_deadline,"conflict resolution did not finish: {:?}",conflicts.snapshot().await);
        let snapshot=conflicts.snapshot().await.unwrap();if terminal_batch(&snapshot.phase){break;}
        let items=store.items(&owner,conflicts.spec.id,0,128).await.unwrap();let mut resolved=false;
        for item in items.into_iter().filter(|item|item.state==FileItemState::WaitingConflict) {
            let choice=if item.spec.relative_name.ends_with("first.bin"){FileConflictPolicy::Replace}else if item.spec.relative_name.ends_with("zero"){FileConflictPolicy::Skip}else{FileConflictPolicy::KeepBoth};
            conflicts.resolve_conflict(item.spec.id,choice).await.unwrap();resolved=true;
        }
        if !resolved {tokio::time::sleep(Duration::from_millis(50)).await;}
    }
    assert_eq!(conflicts.snapshot().await.unwrap().phase,"completed_with_skips");
    assert_eq!(std::fs::read(remote.join("first.bin")).unwrap(),b"new");assert_eq!(std::fs::read(remote.join("nested/中文 file (1).txt")).unwrap(),b"hello");
    let download_spec=spec(reference,FileTransferDirection::Download,&remote,&local_root,FileConflictPolicy::Ask);
    assert!(store.reserve_submission(&owner,&download_spec).await.unwrap());
    let (download,inserted)=ManagedBatchJob::prepare(runtime.clone(),store.clone(),local.clone(),download_spec,owner.clone()).await.unwrap();assert!(!inserted);download.launch();
    let done=settled(&download,&["completed","failed","partial_failed"]).await;assert_eq!(done.phase,"completed","{:?}; {:?}",done.message,download.items(0).await);
    let received=local_root.join("source");assert!(received.join("nested/empty").is_dir());
    assert!(local.browser(format!("batch:{}",download.spec.id)).retained_receipts(None).await.unwrap().is_empty(),"successful directory creation/finalization receipts must not leak retained flags");
    for file in ["first.bin","zero","nested/中文 file.txt","nested/中文 file (1).txt"] {assert_eq!(std::fs::read(received.join(file)).unwrap(),std::fs::read(remote.join(file)).unwrap());}
    for item in download.items(0).await.unwrap().into_iter().filter(|item|matches!(item.spec.kind,ManagedFileKind::File|ManagedFileKind::Directory)) {
        let attributes=item.metadata_result.unwrap();assert_eq!(attributes.actual.modified_at_unix_ms,item.spec.modified_at_unix_ms);
    }
    assert!(store.list("another-account",100).await.unwrap().is_empty());
    let counters:Vec<String>=sqlx::query_scalar("SELECT counters FROM client_usage_accumulators WHERE scope=?").bind(&owner).fetch_all(&runtime.inner.store.pool).await.unwrap();
    let mut usage=UsageCounters::default();for data in counters {usage.add(&serde_json::from_str::<UsageCounters>(&data).unwrap());}
    assert_eq!(usage.uploaded_files,5);assert_eq!(usage.downloaded_files,4);
    // Lose the batch's final directory receipt and terminal update. Recover
    // through the original local/remote mutation journal; never rewrite a
    // directory timestamp that was subsequently changed by someone else.
    while upload.running() || download.running(){tokio::task::yield_now().await;}
    let lost=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(root.join("batches.sqlite3"))).await.unwrap();
    for job in [&upload,&download] {
        let original=job.items(0).await.unwrap().into_iter().find(|item|item.spec.kind==ManagedFileKind::Directory && item.spec.relative_name.ends_with("/empty")).unwrap();
        let attempt=original.attempt.unwrap();assert!(store.directory_finalization(&owner,attempt).await.unwrap().unwrap().result.is_some());
        sqlx::query("UPDATE fm_items SET state='reconciling',worker=NULL,revision=revision+1 WHERE id=?").bind(original.spec.id.to_string()).execute(&lost).await.unwrap();
        sqlx::query("UPDATE fm_directory_finalizations SET result=NULL WHERE attempt=?").bind(attempt.to_string()).execute(&lost).await.unwrap();
        let directory=pab_file_manager::DirectoryObject::open(&original.spec.destination,true).unwrap();
        directory.apply(directory.identity(),&FileTransferMetadata {modified_at_unix_ms:Some(42_000),unix_mode:None}).unwrap();drop(directory);
        job.observe_once_for_test(original.spec.id).await.unwrap();
        let recovered=store.item(&owner,original.spec.id).await.unwrap();assert_eq!(recovered.state,FileItemState::Completed);assert_eq!(recovered.attempt,Some(attempt));assert_eq!(recovered.metadata_result,original.metadata_result);
        assert_eq!(pab_file_manager::DirectoryObject::open(&original.spec.destination,false).unwrap().metadata().unwrap().modified_at_unix_ms,Some(42_000),"original receipt observation must not reapply directory attributes");
    }
    lost.close().await;
    // Inject loss of both the actual-publication fact and terminal task update
    // after atomic rename. Intent alone cannot prove that rename occurred.
    // Reopen the durable store and observe the original attempt;
    // a same-size changed final file must not be called completed or replaced.
    while download.running(){tokio::task::yield_now().await;}
    let published=download.items(0).await.unwrap().into_iter().find(|item|item.spec.relative_name.ends_with("first.bin")).unwrap();
    let attempt=published.attempt.unwrap();let evidence=store.attempt_evidence(&owner,attempt).await.unwrap();assert!(evidence.local_publication_intent && evidence.local_published);assert_eq!(evidence.local_checkpoint,Some(published.spec.size));
    let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(root.join("batches.sqlite3"))).await.unwrap();
    sqlx::query("UPDATE fm_items SET state='reconciling',worker=NULL,revision=revision+1 WHERE id=?").bind(published.spec.id.to_string()).execute(&pool).await.unwrap();
    sqlx::query("UPDATE fm_attempt_evidence SET data=json_remove(data,'$.local_published') WHERE attempt=?").bind(attempt.to_string()).execute(&pool).await.unwrap();pool.close().await;
    std::fs::write(received.join("first.bin"),b"bad").unwrap();
    let reopened=BatchStore::open(&root.join("batches.sqlite3")).await.unwrap();
    runtime.inner.device_codes.lock().await.insert(reference,"987654321".parse().unwrap());
    assert!(ManagedBatchJob::prepare(runtime.clone(),reopened.clone(),local.clone(),spec(reference,FileTransferDirection::Download,&remote,&local_root,FileConflictPolicy::Ask),owner.clone()).await.is_err(),"a new submission must not resolve an obsolete device code");
    let (recovered,inserted)=ManagedBatchJob::prepare(runtime.clone(),reopened.clone(),local.clone(),download.spec.clone(),owner.clone()).await.unwrap();assert!(!inserted);
    recovered.observe_once_for_test(published.spec.id).await.unwrap();assert_eq!(reopened.item(&owner,published.spec.id).await.unwrap().state,FileItemState::Reconciling);assert_eq!(std::fs::read(received.join("first.bin")).unwrap(),b"bad");
    std::fs::write(received.join("first.bin"),b"new").unwrap();recovered.observe_once_for_test(published.spec.id).await.unwrap();
    let observed=reopened.item(&owner,published.spec.id).await.unwrap();assert_eq!(observed.state,FileItemState::Completed);assert_eq!(observed.attempt,Some(attempt));assert_eq!(observed.previous_attempt,published.previous_attempt);
    // A second crash point loses the attribute result as well. Recovery must
    // verify the final bytes and observe attributes without rewriting them.
    let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(root.join("batches.sqlite3"))).await.unwrap();
    sqlx::query("UPDATE fm_items SET state='reconciling',worker=NULL,revision=revision+1 WHERE id=?").bind(published.spec.id.to_string()).execute(&pool).await.unwrap();
    sqlx::query("UPDATE fm_attempt_evidence SET data=json_remove(data,'$.local_metadata_result') WHERE attempt=?").bind(attempt.to_string()).execute(&pool).await.unwrap();pool.close().await;
    let changed_time=std::time::UNIX_EPOCH+Duration::from_secs(10);
    std::fs::File::options().write(true).open(received.join("first.bin")).unwrap().set_times(std::fs::FileTimes::new().set_modified(changed_time)).unwrap();
    recovered.observe_once_for_test(published.spec.id).await.unwrap();
    let observed=reopened.item(&owner,published.spec.id).await.unwrap();assert_eq!(observed.state,FileItemState::Completed);assert_eq!(observed.attempt,Some(attempt));
    assert!(observed.metadata_result.unwrap().warnings.iter().any(|warning|warning.attribute==FileTransferAttribute::ModifiedTime));
    assert_eq!(std::fs::metadata(received.join("first.bin")).unwrap().modified().unwrap(),changed_time);assert_eq!(std::fs::read(received.join("first.bin")).unwrap(),b"new");
    let counters:Vec<String>=sqlx::query_scalar("SELECT counters FROM client_usage_accumulators WHERE scope=?").bind(&owner).fetch_all(&runtime.inner.store.pool).await.unwrap();
    let mut after=UsageCounters::default();for data in counters {after.add(&serde_json::from_str::<UsageCounters>(&data).unwrap());}assert_eq!(after.downloaded_files,usage.downloaded_files,"observation cannot count another successful file");drop((recovered,reopened));runtime.inner.device_codes.lock().await.insert(reference,"123456789".parse().unwrap());
    // Windows denies replacing an explicitly read-only destination. This is a
    // real receiver publication failure, followed by an explicit user retry;
    // the successful sibling must retain its original completed attempt.
    #[cfg(windows)]
    {
        let blocked=remote.join("first.bin");let writable=std::fs::metadata(&blocked).unwrap().permissions();let mut readonly=writable.clone();readonly.set_readonly(true);std::fs::set_permissions(&blocked,readonly).unwrap();
        let retry_spec=spec(reference,FileTransferDirection::Upload,&source,&remote_root,FileConflictPolicy::Replace);
        let (retry,_)=ManagedBatchJob::prepare(runtime.clone(),store.clone(),local.clone(),retry_spec,owner.clone()).await.unwrap();retry.launch();
        let failed=settled(&retry,&["partial_failed","failed","completed"]).await;std::fs::set_permissions(&blocked,writable).unwrap();assert_eq!(failed.phase,"partial_failed","{:?}",retry.items(0).await);
        let before=retry.items(0).await.unwrap();let successful=before.iter().find(|item|item.state==FileItemState::Completed && item.spec.kind==ManagedFileKind::File).unwrap().clone();let failed=before.iter().find(|item|item.state==FileItemState::Failed && item.spec.kind==ManagedFileKind::File).unwrap().clone();
        let deferred=before.iter().find(|item|item.state==FileItemState::Failed && item.spec.kind==ManagedFileKind::Directory).unwrap();assert!(deferred.metadata_result.is_none());
        let ready=store.directory_ready(&owner,deferred.spec.id).await.unwrap().unwrap();
        while retry.running(){tokio::task::yield_now().await;}
        retry.retry_failed().await.unwrap();assert_eq!(settled(&retry,&["completed","partial_failed","failed"]).await.phase,"completed");
        assert_eq!(store.item(&owner,successful.spec.id).await.unwrap(),successful);
        assert_eq!(store.directory_ready(&owner,deferred.spec.id).await.unwrap().unwrap(),ready);assert!(store.item(&owner,deferred.spec.id).await.unwrap().metadata_result.is_some());
        let repaired=store.item(&owner,failed.spec.id).await.unwrap();assert_ne!(repaired.attempt,failed.attempt);assert_eq!(repaired.previous_attempt,failed.attempt);
        assert_eq!(std::fs::read(&blocked).unwrap(),b"new");while retry.running(){tokio::task::yield_now().await;}drop(retry);
        let local_target=received.join("first.bin");let writable=std::fs::metadata(&local_target).unwrap().permissions();let mut readonly=writable.clone();readonly.set_readonly(true);std::fs::set_permissions(&local_target,readonly).unwrap();
        let (retry,_)=ManagedBatchJob::prepare(runtime.clone(),store.clone(),local.clone(),spec(reference,FileTransferDirection::Download,&blocked,&received,FileConflictPolicy::Replace),owner.clone()).await.unwrap();retry.launch();
        let failed=settled(&retry,&["failed","partial_failed","completed"]).await;std::fs::set_permissions(&local_target,writable).unwrap();assert_eq!(failed.phase,"failed","{:?}",retry.items(0).await);
        let previous=retry.items(0).await.unwrap().remove(0).attempt;
        while retry.running(){tokio::task::yield_now().await;}retry.retry_failed().await.unwrap();assert_eq!(settled(&retry,&["completed","failed"]).await.phase,"completed");
        assert_eq!(retry.items(0).await.unwrap()[0].previous_attempt,previous);assert_eq!(std::fs::read(local_target).unwrap(),b"new");
        while retry.running(){tokio::task::yield_now().await;}drop(retry);
    }
    // A batch freezes its root at admission, before scanning and file attempts.
    // A replaced root is rejected in both directions and cannot be rebound by
    // reopening the same batch. Restoring the original enables explicit retry.
    for direction in [FileTransferDirection::Upload,FileTransferDirection::Download] {
        let fixture=root.join(format!("parent-replacement-{direction:?}"));std::fs::create_dir(&fixture).unwrap();
        let source=fixture.join("source.bin");std::fs::write(&source,b"original source").unwrap();let destination=fixture.join("destination");std::fs::create_dir(&destination).unwrap();
        let request=spec(reference,direction,&source,&destination,FileConflictPolicy::Replace);
        let (job,_)=ManagedBatchJob::prepare(runtime.clone(),store.clone(),local.clone(),request.clone(),owner.clone()).await.unwrap();
        std::fs::rename(&destination,fixture.join("original")).unwrap();std::fs::create_dir(&destination).unwrap();std::fs::write(destination.join("source.bin"),b"replacement contents").unwrap();
        assert!(ManagedBatchJob::prepare(runtime.clone(),store.clone(),local.clone(),request,owner.clone()).await.is_err());
        job.launch();assert_eq!(settled(&job,&["failed","completed","partial_failed"]).await.phase,"failed");
        let failed=job.items(0).await.unwrap().remove(0);assert_eq!(failed.confirmed.0,0);assert!(failed.message.as_deref().is_some_and(|message|message.contains("parent changed")),"{:?}",failed.message);
        assert_eq!(std::fs::read(destination.join("source.bin")).unwrap(),b"replacement contents");assert_eq!(std::fs::read_dir(&destination).unwrap().count(),1);
        while job.running(){tokio::task::yield_now().await;}
        std::fs::rename(&destination,fixture.join("replacement")).unwrap();std::fs::rename(fixture.join("original"),&destination).unwrap();
        job.retry_failed().await.unwrap();assert_eq!(settled(&job,&["failed","completed"]).await.phase,"completed");
        assert_eq!(std::fs::read(destination.join("source.bin")).unwrap(),b"original source");assert_eq!(std::fs::read(fixture.join("replacement/source.bin")).unwrap(),b"replacement contents");
        while job.running(){tokio::task::yield_now().await;}drop(job);
    }
    // Same-host connections are real filesystem copies despite traversing the
    // network protocol. A rejected root must not block an unrelated valid item.
    for direction in [FileTransferDirection::Upload,FileTransferDirection::Download] {
        for mode in ["same-directory","descendant","same-file","hardlink"] {
            let fixture=root.join(format!("self-copy-{direction:?}-{mode}"));
            let selected=fixture.join("selected");let separate=fixture.join("separate");
            std::fs::create_dir_all(selected.join("inside")).unwrap();std::fs::create_dir(&separate).unwrap();
            let bad_file=selected.join("original.bin");std::fs::write(&bad_file,b"source remains intact").unwrap();
            let good_file=separate.join("allowed.bin");std::fs::write(&good_file,b"independent source").unwrap();
            let (bad,destination)=match mode {
                "same-directory"=>(selected.clone(),fixture.clone()),
                "descendant"=>(selected.clone(),selected.join("inside")),
                "same-file"=>(bad_file.clone(),selected.clone()),
                _=>{let destination=fixture.join("hardlink-target");std::fs::create_dir(&destination).unwrap();std::fs::hard_link(&bad_file,destination.join("original.bin")).unwrap();(bad_file.clone(),destination)},
            };
            let mut request=spec(reference,direction,&bad,&destination,FileConflictPolicy::Replace);
            request.sources.push(encode_path(&good_file));
            let (job,_)=ManagedBatchJob::prepare(runtime.clone(),store.clone(),local.clone(),request,owner.clone()).await.unwrap();job.launch();
            assert_eq!(settled(&job,&["partial_failed","completed","failed"]).await.phase,"partial_failed","{direction:?} {mode}");
            let items=job.items(0).await.unwrap();
            let refused=items.iter().find(|item|item.spec.source==encode_path(&bad)).unwrap();
            assert_eq!(refused.state,FileItemState::Failed);assert_eq!(refused.confirmed.0,0);
            assert!(refused.message.as_deref().is_some_and(|message|message.contains("same filesystem object")||message.contains("descendants")),"{:?}",refused.message);
            assert_eq!(items.iter().find(|item|item.spec.source==encode_path(&good_file)).unwrap().state,FileItemState::Completed);
            assert_eq!(std::fs::read(destination.join("allowed.bin")).unwrap(),b"independent source");
            assert_eq!(std::fs::read(&bad_file).unwrap(),b"source remains intact");
            if mode=="descendant" {assert!(!destination.join("selected").exists());}
            while job.running(){tokio::task::yield_now().await;}drop(job);
        }
    }
    while upload.running() || conflicts.running() || download.running(){tokio::time::sleep(Duration::from_millis(20)).await;}
    drop((upload,conflicts,download));stop.send_replace(true);let mut failures=receiver.await.unwrap();
    assert_eq!(failures.iter().filter(|error|error.contains("parent changed")).count(),1,"{failures:?}");failures.retain(|error|!error.contains("parent changed"));
    #[cfg(windows)]
    assert!(failures.len()==1 && failures[0].contains("os error 5"),"only the injected read-only publication may fail: {failures:?}");
    #[cfg(not(windows))]
    assert!(failures.is_empty(),"unexpected stream failures: {failures:?}");
    runtime.inner.file_devices.lock().await.clear();runtime.inner.devices.lock().await.clear();
    shutdown_fixture(runtime).await;first.close().await;second.close().await;drop((store,local));
    super::file_fixture_cleanup::cleanup_fixture(temp).await.unwrap();
}
fn terminal_batch(phase:&str)->bool {matches!(phase,"completed"|"completed_with_skips"|"partial_failed"|"failed"|"cancelled")}

#[tokio::test]
async fn managed_batch_stop_waits_for_receiver_checkpoint_then_resumes_without_replacing_early() {
    for direction in [FileTransferDirection::Upload,FileTransferDirection::Download] {
        Box::pin(stop_case(FileTransferIntent::Pause,direction,false,false,false)).await;
        Box::pin(stop_case(FileTransferIntent::Cancel,direction,false,false,false)).await;
    }
}
#[tokio::test]
async fn ending_an_account_session_stops_background_work_and_requires_original_attempt_reconciliation() {
    Box::pin(stop_case(FileTransferIntent::Pause,FileTransferDirection::Download,true,false,false)).await;
}
#[tokio::test]
async fn confirmed_shutdown_releases_the_writer_without_faking_a_pause_and_can_reconcile_next_start() {
    Box::pin(stop_case(FileTransferIntent::Pause,FileTransferDirection::Download,true,true,false)).await;
}
#[tokio::test]
async fn a_coordinator_database_error_survives_revocation_and_preserves_the_original_attempt() {
    Box::pin(stop_case(FileTransferIntent::Pause,FileTransferDirection::Download,true,false,true)).await;
}
async fn stop_case(intent:FileTransferIntent,direction:FileTransferDirection,end_session:bool,shutdown:bool,fault:bool) {
    let temp=fixture_directory("pab-batch-stop-");let root=temp.path().canonicalize().unwrap();
    let source=root.join("payload.bin");let remote=root.join("remote");std::fs::create_dir(&remote).unwrap();
    let data:Vec<u8>=(0..4_194_305).map(|n|(n%251)as u8).collect();std::fs::write(&source,&data).unwrap();
    let (source,destination,target)=if direction==FileTransferDirection::Download {
        let source=remote.join("payload.bin");std::fs::write(&source,&data).unwrap();let destination=root.join("received");std::fs::create_dir(&destination).unwrap();let target=destination.join("payload.bin");(source,destination,target)
    }else{(source,remote.clone(),remote.join("payload.bin"))};
    std::fs::write(&target,b"original destination").unwrap();
    let (mut runtime,_credentials)=test_support::runtime(&root).await;let origin="https://file-stop-fixture.invalid";
    Arc::get_mut(&mut Arc::get_mut(&mut runtime).unwrap().inner).unwrap().store.account_origin=Some(origin.into());
    sqlx::query("INSERT INTO runtime_session_servers(session_id,origin) VALUES('fixture',?)").bind(origin).execute(&runtime.inner.store.pool).await.unwrap();
    let owner=runtime.file_operation_owner().await.unwrap();
    let (first,second,client,server)=crate::connection::test_connection_pair().await;let reference=client.device_ref();let actor=client.operator();
    runtime.inner.device_codes.lock().await.insert(reference,"123456789".parse().unwrap());
    runtime.inner.file_device(reference,false).await.unwrap().set_test_connection(client).await;
    let barrier=pab_executor::test_support::UploadBarrier::new(remote.clone(),65_536);
    let publication=crate::connection::PublicationBarrier::new();
    let service=pab_executor::test_support::ProtocolService::open(&root.join("executor.sqlite3"),reference,pab_platform::detect_native_execution_context().unwrap()).await.unwrap().with_upload_barrier(barrier.clone());
    let observer=service.clone();let (stop,mut stopped)=watch::channel(false);
    let receiver=tokio::spawn(async move {
        let mut tasks=tokio::task::JoinSet::new();
        let mut failures=Vec::new();
        loop {tokio::select! {
            _=stopped.changed()=>break,
            stream=server.accept_bi(Duration::from_secs(30))=>{let Ok(stream)=stream else {break};let service=service.clone();tasks.spawn(async move {Box::pin(service.handle_stream(actor,stream,Duration::from_secs(10))).await});},
            Some(result)=tasks.join_next(),if !tasks.is_empty()=>{if let Err(error)=result.unwrap(){failures.push(error);}},
        }}
        while let Some(result)=tasks.join_next().await {if let Err(error)=result.unwrap(){failures.push(error);}}
        failures
    });
    let store=BatchStore::open(&root.join("batches.sqlite3")).await.unwrap();let local=Arc::new(LocalFileManager::open(&root.join("local.sqlite3")).await.unwrap());
    let (job,_)=ManagedBatchJob::prepare(runtime.clone(),store.clone(),local.clone(),spec(reference,direction,&source,&destination,FileConflictPolicy::Replace),owner.clone()).await.unwrap();
    if direction==FileTransferDirection::Download && intent==FileTransferIntent::Pause {
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(root.join("batches.sqlite3"))).await.unwrap();
        job.delay_publication_writer_for_test(pool);
    }
    if direction==FileTransferDirection::Download {job.set_publication_barrier(publication.clone());}
    job.launch();
    tokio::time::timeout(Duration::from_secs(15),async {if direction==FileTransferDirection::Upload {barrier.reached().await;}else{publication.reached().await;}}).await.expect("receiver did not reach checkpoint");
    if end_session {
        let account=runtime.inner.account_store.read().unwrap();
        if fault {
            let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(root.join("batches.sqlite3"))).await.unwrap();
            sqlx::query("DROP TABLE fm_dependencies").execute(&pool).await.unwrap();pool.close().await;
        }else if shutdown {job.request_shutdown();}else{runtime.inner.account_store.logout().unwrap();}
        let cancelled=tokio::time::timeout(Duration::from_secs(5),publication.cancelled()).await;publication.release();barrier.release();cancelled.expect("account revocation never stopped the local receiver");
        tokio::time::timeout(Duration::from_secs(5),async {while job.running(){tokio::time::sleep(Duration::from_millis(10)).await;}}).await.expect("revoked coordinator retained its heartbeat");
        let snapshot=job.snapshot().await.unwrap();assert_eq!(snapshot.intent,FileTransferIntent::Pause);assert_eq!(snapshot.phase,"reconciling");assert!(!job.session_current());assert!(job.request_intent(FileTransferIntent::Run).await.is_err());
        let elapsed=snapshot.active_elapsed_ms;
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(job.snapshot().await.unwrap().active_elapsed_ms,elapsed,"account/downtime wait must not accumulate elapsed time");
        let original=job.items(0).await.unwrap().remove(0);let attempt=original.attempt.unwrap();assert_eq!(std::fs::read(&target).unwrap(),b"original destination");
        // A process restart must leave this coordinator's original attempt
        // out of legacy history recovery and its shared desktop connection.
        sqlx::query("UPDATE runtime_sessions SET heartbeat_at_unix_ms=0 WHERE id='fixture'").execute(&runtime.inner.store.pool).await.unwrap();
        assert!(runtime.inner.store.unconfirmed_transfers(8,None).await.unwrap().is_empty());assert!(runtime.cancel_transfer_record(attempt).await.is_err());
        sqlx::query("UPDATE runtime_sessions SET heartbeat_at_unix_ms=? WHERE id='fixture'").bind(super::unix_millis()).execute(&runtime.inner.store.pool).await.unwrap();
        if fault {
            assert!(job.items(0).await.unwrap()[0].message.as_deref().is_some_and(|message|message.contains("fm_dependencies")),"original storage failure was replaced by a generic revocation");
            drop(BatchStore::open(&root.join("batches.sqlite3")).await.unwrap());
        }
        if !shutdown && !fault {
            let logged_in=runtime.inner.account_store.login(pab_agent_core::account::AccountSession {user:account.user.unwrap(),access_token:"fixture-token".into()}).unwrap();
            let mut presence=runtime.inner.presence.lock().unwrap();let sync=presence.account.as_mut().unwrap();sync.local_revision=logged_in.revision;sync.server_revision=Some(logged_in.revision);
        }
        assert!(!job.session_current(),"re-login cannot reactivate a coordinator from the old account revision");
        let (recovered,inserted)=ManagedBatchJob::prepare(runtime.clone(),store.clone(),local.clone(),job.spec.clone(),owner.clone()).await.unwrap();assert!(!inserted);
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(root.join("batches.sqlite3"))).await.unwrap();recovered.delay_publication_writer_for_test(pool);recovered.launch();
        assert_eq!(settled(&recovered,&["paused","failed"]).await.phase,"paused");while recovered.running(){tokio::task::yield_now().await;}
        assert_eq!(recovered.snapshot().await.unwrap().active_elapsed_ms,elapsed,"observing the original attempt must not charge reconnect wait");
        recovered.request_intent(FileTransferIntent::Run).await.unwrap();assert_eq!(settled(&recovered,&["completed","failed"]).await.phase,"completed");assert_eq!(std::fs::read(&target).unwrap(),data);assert_eq!(recovered.items(0).await.unwrap()[0].previous_attempt,Some(attempt));
        while recovered.running(){tokio::task::yield_now().await;}drop((job,recovered));stop.send_replace(true);assert_expected_stop_streams(receiver.await.unwrap());runtime.inner.file_devices.lock().await.clear();runtime.inner.devices.lock().await.clear();
        shutdown_fixture(runtime).await;first.close().await;second.close().await;drop((observer,store,local));super::file_fixture_cleanup::cleanup_fixture(temp).await.unwrap();return;
    }
    let requested=job.request_intent(intent).await.unwrap();
    assert_eq!(requested.phase,if intent==FileTransferIntent::Pause {"pause_requested"}else{"cancel_requested"});
    let original=job.items(0).await.unwrap().remove(0);let attempt=original.attempt.unwrap();
    let acknowledged=tokio::time::timeout(Duration::from_secs(5),async {if direction==FileTransferDirection::Download {publication.cancelled().await;}else{loop {if observer.stop_pending(attempt).await.unwrap(){break;}tokio::time::sleep(Duration::from_millis(10)).await;}}}).await;
    // Always release before asserting, so an assertion cannot strand the receiver.
    barrier.release();publication.release();acknowledged.expect("stop request never reached receiver");
    let expected=if intent==FileTransferIntent::Pause {"paused"}else{"cancelled"};
    assert_eq!(settled(&job,&[expected,"failed","partial_failed"]).await.phase,expected);
    assert_eq!(std::fs::read(&target).unwrap(),b"original destination");
    let evidence=store.attempt_evidence(&owner,attempt).await.unwrap();
    #[cfg(unix)]
    if direction==FileTransferDirection::Download {
        let staging=evidence.local_staging.as_ref().expect("managed download must persist its original receiving ownership");
        assert_eq!(staging.destination,pab_file_manager::encode_path(&target));
        let payload=target.parent().unwrap().join(&staging.directory_name).join("payload");
        if intent==FileTransferIntent::Cancel {assert!(evidence.local_staging_discarded);assert!(!payload.exists());}
        else {assert_eq!(std::fs::read(payload).unwrap(),data);}
    }
    if intent==FileTransferIntent::Cancel {
        let retained:Vec<_>=std::fs::read_dir(target.parent().unwrap()).unwrap().map(|entry|entry.unwrap().file_name()).filter(|name|name.to_string_lossy().ends_with(".part")).collect();
        assert!(retained.is_empty(),"cancelled receiving data was not removed: {retained:?}");
        if direction==FileTransferDirection::Download {assert!(evidence.local_staging_discarded);}
    }
    let receipt=evidence.receipt.unwrap();
    assert_eq!(receipt.state,if direction==FileTransferDirection::Upload {expected}else{"completed"});
    if direction==FileTransferDirection::Upload {assert_eq!(receipt.published,Some(false));}else{assert_eq!(evidence.local_checkpoint,Some(FileByteCount(data.len() as u64)));}
    assert!(receipt.offset>=65_536);
    while job.running(){tokio::task::yield_now().await;}
    let elapsed=job.snapshot().await.unwrap().active_elapsed_ms;assert!(elapsed.0>0);
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(job.snapshot().await.unwrap().active_elapsed_ms,elapsed,"paused/cancelled time must remain frozen");
    let reopened=BatchStore::open(&root.join("batches.sqlite3")).await.unwrap();
    assert_eq!(reopened.snapshot(&owner,job.spec.id).await.unwrap().active_elapsed_ms,elapsed);drop(reopened);
    if intent==FileTransferIntent::Pause {
        while job.running(){tokio::task::yield_now().await;}
        job.request_intent(FileTransferIntent::Run).await.unwrap();
        assert_eq!(settled(&job,&["completed","failed","partial_failed"]).await.phase,"completed");
        assert_eq!(std::fs::read(&target).unwrap(),data);
        let resumed=job.items(0).await.unwrap().remove(0);assert_ne!(resumed.attempt,Some(attempt));assert_eq!(resumed.previous_attempt,Some(attempt));
        #[cfg(unix)]
        if direction==FileTransferDirection::Download {
            let old=store.attempt_evidence(&owner,attempt).await.unwrap();let new=store.attempt_evidence(&owner,resumed.attempt.unwrap()).await.unwrap();
            assert_eq!(new.local_staging,old.local_staging,"resume must retain the original receiving object, not adopt a similarly named file");
            let staging=new.local_staging.unwrap();assert!(!target.parent().unwrap().join(staging.directory_name).join("payload").exists());
        }
        assert_eq!(store.attempt_evidence(&owner,attempt).await.unwrap().receipt.unwrap().state,if direction==FileTransferDirection::Upload {"paused"}else{"completed"});
        while job.running(){tokio::task::yield_now().await;}
        assert!(job.snapshot().await.unwrap().active_elapsed_ms.0>elapsed.0,"resuming must add a new working segment to the original batch");
    }
    while job.running(){tokio::task::yield_now().await;}drop(job);stop.send_replace(true);assert_expected_stop_streams(receiver.await.unwrap());
    runtime.inner.file_devices.lock().await.clear();runtime.inner.devices.lock().await.clear();
    shutdown_fixture(runtime).await;first.close().await;second.close().await;drop((observer,store,local));
    super::file_fixture_cleanup::cleanup_fixture(temp).await.unwrap();
}
async fn shutdown_fixture(runtime:Arc<BridgeRuntime>) {
    tokio::time::timeout(Duration::from_secs(5),async {while Arc::strong_count(&runtime)!=1 {tokio::task::yield_now().await;}}).await.expect("batch tasks leaked runtime ownership");
    runtime.inner.account_store.clear_local_session().unwrap();
    Arc::try_unwrap(runtime).ok().unwrap().shutdown().await.unwrap();
}
