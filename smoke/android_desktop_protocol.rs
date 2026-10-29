//! Bounded real-device verification of the protocols used by Desktop windows.
//! Uses the normal account and saved pairing; writes only its unique workspace folder.
use pab_bridge::{AuthenticatedDeviceConnection,BridgeConfig,BridgeRuntime,BridgeRuntimeConfig,SqliteDevicePasswordProvider,TransferControl};
use pab_protocol::*;
use std::{sync::Arc,time::Duration};
type Error=Box<dyn std::error::Error+Send+Sync>;
fn main()->Result<(),Error> {
    let (args,values)=pab_service_config::client::startup(pab_service_config::client::Role::Operator,std::env::args_os().skip(1))?;
    for (key,value) in values {unsafe {std::env::set_var(key,value);}}
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?.block_on(run(args))
}
async fn query(connection:&AuthenticatedDeviceConnection,operation:FileManagerOperation)->Result<FileManagerReply,Error> {
    let request=FileManagerRequest {schema_version:1,request_id:RequestId::new(),execution:Default::default(),expected_execution:None,operation};
    let mut reply=connection.file_manager(&request).await?.reply;
    for _ in 0..100 {
        if reply.state!="running" {break;}
        tokio::time::sleep(Duration::from_millis(100)).await;
        reply=connection.get_filesystem(request.request_id).await?;
    }
    if reply.state!="completed" {return Err(format!("managed operation failed: {:?}",reply.error).into());}
    let body=reply.file_manager.ok_or("missing managed result")?;body.validate_for(&request)?;Ok(body)
}
async fn run(args:Vec<String>)->Result<(),Error> {
    if args.len()!=1 {return Err("usage: android_desktop_protocol [--config FILE] DEVICE_CODE".into());}
    let temp=tempfile::tempdir()?;
    let config=BridgeConfig::register_guest_with_secret(temp.path().join("operator.key")).await?;
    let passwords=Arc::new(SqliteDevicePasswordProvider::new(pab_agent_core::DataPaths::for_operator()?.bridge_database()));
    let mut options=BridgeRuntimeConfig::new(temp.path().join("runtime.sqlite3"));options.resume_incomplete_on_start=false;
    let runtime=Arc::new(BridgeRuntime::start(config,options,passwords).await?);
    tokio::time::timeout(Duration::from_secs(120),async {
        let device=runtime.resolve_device_code(args[0].parse()?).await?;
        let connection=runtime.file_connection(device,false).await?;connection.require_managed_transfer().await?;
        let locations=query(&connection,FileManagerOperation::Locations {}).await?;
        let context=locations.execution_context.ok_or("missing app execution context")?;
        if context.os_family!=OsFamily::Android {return Err("This probe only targets Android".into());}
        let identity=FileExecutionIdentity::from_context(&context)?;
        let FileManagerResult::Locations {locations}=locations.result else{return Err("missing locations".into())};
        if locations.home!=Some("/workspace".into()) {return Err("unexpected workspace".into());}
        let name=format!("desktop-smoke-{}",RequestId::new());let folder=format!("/workspace/{name}");
        query(&connection,FileManagerOperation::CreateDirectory {parent:"/workspace".into(),name:name.into()}).await?;
        let result=async {
            batch_roundtrip(runtime.clone(), device, &folder, temp.path()).await?;
            let parent=query(&connection,FileManagerOperation::InspectDirectoryObject {path:folder.clone().into()}).await?;
            let FileManagerResult::DirectoryObject {identity:parent,..}=parent.result else{return Err("missing parent identity".into())};
            let source=temp.path().join("source.bin");let bytes:Vec<u8>=(0..1024*1024).map(|n|(n%251)as u8).collect();std::fs::write(&source,&bytes)?;
            let path:NativeFilePath=format!("{folder}/roundtrip.bin").into();
            let upload_options=FileTransferOptions {expected_execution:Some(identity.clone()),destination_parent:Some(FileTransferParentConstraint {identity:parent}),metadata:Some(FileTransferMetadata {modified_at_unix_ms:Some(1_700_000_000_000),unix_mode:None}),..Default::default()};
            let id=RequestId::new();
            connection.upload_native_file_with_options(id,&source,&path,false,&upload_options,&TransferControl::default(),|_,_|{}).await?;
            let receipt=connection.get_transfer(id).await?;
            if receipt.state!="completed" || receipt.published!=Some(true) || receipt.metadata_result.is_none(){return Err("upload receipt incomplete".into());}
            let target=temp.path().join("received.bin");
            connection.download_native_file_with_options(RequestId::new(),&path,&target,false,&FileTransferOptions {expected_execution:Some(identity.clone()),source_constraint:Some(FileTransferSourceConstraint {size:FileByteCount(bytes.len()as u64),modified_at_unix_ms:None}),..Default::default()},receipt.sha256.as_deref(),&TransferControl::default(),|_,_|{}).await?;
            if std::fs::read(target)?!=bytes {return Err("roundtrip bytes differ".into());}
            if connection.upload_native_file_with_options(RequestId::new(),&source,&path,false,&upload_options,&TransferControl::default(),|_,_|{}).await.is_ok(){return Err("conflict silently overwritten".into());}
            let opened=query(&connection,FileManagerOperation::OpenDirectory {path:folder.clone().into()}).await?;
            let FileManagerResult::Opened {snapshot_id}=opened.result else{return Err("missing directory snapshot".into())};
            let page=query(&connection,FileManagerOperation::ReadDirectory {snapshot_id,after:None,limit:32,filter:Default::default()}).await?;
            let FileManagerResult::Directory {page}=page.result else{return Err("missing directory page".into())};
            if page.entries.len()!=1 || page.entries[0].name!="roundtrip.bin" {return Err("directory content differs".into());}
            query(&connection,FileManagerOperation::CloseDirectory {snapshot_id}).await?;
            query(&connection,FileManagerOperation::Rename {path:path.clone(),name:"renamed.bin".into()}).await?;
            println!("FILE_PASS: locations, app identity, pinned upload, metadata receipt, download/hash, conflict, listing, rename");
            let terminal=runtime.open_desktop_terminal(device,Default::default(),80,24).await?;
            terminal.resize(100,32).await?;
            terminal.input(b"printf 'PIXELS_%s\\n' 'ANDROID_TERMINAL_OK'; stty size\n".to_vec()).await?;
            let mut output=Vec::new();
            tokio::time::timeout(Duration::from_secs(15),async {
                loop {
                    match terminal.next_event().await? {
                        DesktopTerminalEvent::Output {offset,bytes}=>{let end=offset+bytes.len()as u64;output.extend(bytes);terminal.consume(end).await?;let text=String::from_utf8_lossy(&output);if text.contains("PIXELS_ANDROID_TERMINAL_OK") && text.contains("32 100"){break;}}
                        _=>return Err::<(),Error>("terminal ended before command output".into()),
                    }
                }
                Ok::<(),Error>(())
            }).await??;
            terminal.close().await?;
            println!("TERMINAL_PASS: app UID, input, streamed output/ACK, resize, close");
            Ok::<(),Error>(())
        }.await;
        let cleanup=query(&connection,FileManagerOperation::Delete {path:folder.into(),recursive:true}).await;
        result?;cleanup?;println!("CLEANUP_PASS");Ok::<(),Error>(())
    }).await??;
    Arc::try_unwrap(runtime).map_err(|_|"probe runtime still owned")?.shutdown().await?;Ok(())
}

async fn batch_roundtrip(runtime:Arc<BridgeRuntime>,device:DeviceRef,folder:&str,root:&std::path::Path)->Result<(),Error> {
    use pab_file_manager::{BatchStore,LocalFileManager,encode_path};
    let owner=runtime.file_operation_owner().await?;
    let store=BatchStore::open(&root.join("batch.sqlite3")).await?;
    let local=Arc::new(LocalFileManager::open(&root.join("local.sqlite3")).await?);
    let source=root.join("batch-tree");std::fs::create_dir_all(source.join("nested"))?;
    let bytes=vec![83u8;41550];std::fs::write(source.join("image.bin"),&bytes)?;std::fs::write(source.join("nested/中文.txt"),"batch roundtrip 中文")?;
    let file=root.join("single.bin");std::fs::write(&file,&bytes)?;
    let code="919383159".parse()?;
    let spec=FileBatchSpec {id:RequestId::new(),device,code,direction:FileTransferDirection::Upload,sources:vec![encode_path(&file),encode_path(&source)],destination:folder.into(),execution:ExecutionSelection::Service {},conflict:FileConflictPolicy::Ask,force_relay:false};
    let (job,_)=pab_bridge::ManagedBatchJob::prepare(runtime.clone(),store.clone(),local.clone(),spec,owner.clone()).await?;job.launch();
    wait_batch(&job).await?;
    let received=root.join("batch-received");std::fs::create_dir(&received)?;
    let spec=FileBatchSpec {id:RequestId::new(),device,code,direction:FileTransferDirection::Download,sources:vec![format!("{folder}/single.bin").into(),format!("{folder}/batch-tree").into()],destination:encode_path(&received),execution:ExecutionSelection::Service {},conflict:FileConflictPolicy::Ask,force_relay:false};
    let (job,_)=pab_bridge::ManagedBatchJob::prepare(runtime.clone(),store,local,spec,owner).await?;job.launch();wait_batch(&job).await?;
    assert_eq!(std::fs::read(received.join("single.bin"))?,bytes);assert_eq!(std::fs::read(received.join("batch-tree/image.bin"))?,bytes);assert_eq!(std::fs::read_to_string(received.join("batch-tree/nested/中文.txt"))?,"batch roundtrip 中文");
    let connection=runtime.file_connection(device,false).await?;
    for path in [format!("{folder}/single.bin"),format!("{folder}/batch-tree")] {query(&connection,FileManagerOperation::Delete {path:path.into(),recursive:true}).await?;}
    println!("BATCH_PASS: Desktop ManagedBatchJob single file + nested directory upload/download, byte equality, cleanup");Ok(())
}
async fn wait_batch(job:&Arc<pab_bridge::ManagedBatchJob>)->Result<(),Error> {
    tokio::time::timeout(Duration::from_secs(45),async {
        loop {let snapshot=job.snapshot().await?;
            if !job.running() {if snapshot.phase=="completed" {return Ok::<(),Error>(());}return Err(format!("batch failed: {:?}",snapshot).into());}
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }).await?
}
