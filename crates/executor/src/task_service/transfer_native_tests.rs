use super::*;
#[tokio::test]
async fn expected_native_identity_is_checked_before_acceptance_or_staging_effects() {
    let root=tempfile::tempdir().unwrap();let service=transfer_tests::service(root.path()).await;let destination=root.path().join("protected.bin");std::fs::write(&destination,b"original destination").unwrap();let native=pab_file_manager::encode_path(&destination);
    let mut expected=pab_protocol::FileExecutionIdentity::from_context(&service.execution_context).unwrap();expected.identity.account_id.push_str("-different-account");
    let request=FileTransferRequest { destination_parent:None,source_constraint:None,metadata:None,expected_execution:Some(expected),request_id:RequestId::new(),native_path:Some(native.clone()),execution:Default::default(),resume_from:None,operation:FileTransferOperation::Upload {path:native.display_label(),size:4,sha256:"0".repeat(64),overwrite:true}};
    let (first,second,client,server)=transfer_tests::pair().await;let executor=service.clone();let running=tokio::spawn(async move {let stream=server.accept_bi(TIMEOUT).await.unwrap();executor.handle_stream(transfer_tests::actor(),stream,TIMEOUT).await});
    let mut stream=client.open_bi(TIMEOUT).await.unwrap();stream.send_frame_json(&DeviceTaskRequest::TransferFile {schema_version:DEVICE_TASK_SCHEMA_VERSION,request:request.clone()},TIMEOUT).await.unwrap();
    let reply=stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap();assert!(matches!(reply,DeviceTaskResponse::Error {message,..} if message.contains("before acceptance")));assert!(matches!(running.await.unwrap(),Err(TaskServiceError::ExecutionContext(_))));
    assert!(service.store.existing_transfer(transfer_tests::actor(),&request).await.unwrap().is_none());assert_eq!(std::fs::read(&destination).unwrap(),b"original destination");assert!(!std::fs::read_dir(root.path()).unwrap().any(|entry|entry.unwrap().file_name().to_string_lossy().contains(".part")));drop(stream);first.close().await;second.close().await;
}
use pab_protocol::{FileTransferOperation, FileTransferRequest, RequestId};
const TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn persisted_parent_prevents_recovery_from_an_identical_replacement_after_restart() {
    for attributes in [false,true] {
        let root=tempfile::tempdir().unwrap();let parent=root.path().join("receiver");std::fs::create_dir(&parent).unwrap();
        let constraint=pab_protocol::FileTransferParentConstraint {identity:pab_file_manager::DirectoryObject::open(&pab_file_manager::encode_path(&parent),false).unwrap().identity().to_owned()};
        let target=parent.join("result");let native=pab_file_manager::encode_path(&target);let bytes=b"right";
        let request=FileTransferRequest {destination_parent:Some(constraint),request_id:RequestId::new(),metadata:attributes.then(pab_protocol::FileTransferMetadata::default),source_constraint:None,expected_execution:None,native_path:Some(native.clone()),execution:Default::default(),resume_from:None,operation:FileTransferOperation::Upload {path:native.display_label(),size:5,sha256:format!("{:x}",Sha256::digest(bytes)),overwrite:false}};
        let service=transfer_tests::service(root.path()).await;
        service.store.accept_transfer(transfer_tests::actor(),&request,&service.execution_context).await.unwrap();service.store.transfer_progress(request.request_id,5,5).await.unwrap();service.store.begin_transfer_publication(request.request_id).await.unwrap();
        std::fs::write(&target,bytes).unwrap();drop(service);
        std::fs::rename(&parent,root.path().join("original")).unwrap();std::fs::create_dir(&parent).unwrap();std::fs::write(&target,bytes).unwrap();
        let reopened=transfer_tests::service(root.path()).await;
        let pending=reopened.lookup_transfer(transfer_tests::actor(),request.request_id).await.unwrap();assert_eq!(pending.state,"committing");assert!(pending.metadata_result.is_none());assert_ne!(pending.published,Some(true));
        assert_eq!(reopened.store.transfer_request(transfer_tests::actor(),request.request_id).await.unwrap().destination_parent,request.destination_parent);
        assert_eq!(std::fs::read(&target).unwrap(),bytes);
        std::fs::rename(&parent,root.path().join("replacement")).unwrap();std::fs::rename(root.path().join("original"),&parent).unwrap();
        let result=reopened.lookup_transfer(transfer_tests::actor(),request.request_id).await.unwrap();assert_eq!(result.state,"completed");assert_eq!(result.published,Some(true));
        assert_eq!(std::fs::read(root.path().join("replacement/result")).unwrap(),bytes);
    }
}

#[tokio::test]
async fn service_attribute_recovery_waits_for_the_original_writer_and_verifies_bytes_without_mutation() {
    use std::io::Write;
    use pab_protocol::{FileTransferMetadata,FileTransferAttribute};
    for published_fact in [false,true] {
        let root=tempfile::tempdir().unwrap();let service=transfer_tests::service(root.path()).await;
        let target=root.path().join("final.bin");let native=pab_file_manager::encode_path(&target);let bytes=b"right";
        let metadata=FileTransferMetadata {modified_at_unix_ms:Some(1_700_000_000_123),unix_mode:Some(0o640)};
        let request=FileTransferRequest { destination_parent:None,request_id:RequestId::new(),metadata:Some(metadata.clone()),source_constraint:None,expected_execution:None,native_path:Some(native.clone()),execution:Default::default(),resume_from:None,operation:FileTransferOperation::Upload {path:native.display_label(),size:5,sha256:format!("{:x}",Sha256::digest(bytes)),overwrite:false}};
        service.store.accept_transfer(transfer_tests::actor(),&request,&service.execution_context).await.unwrap();service.store.transfer_progress(request.request_id,5,5).await.unwrap();service.store.begin_transfer_publication(request.request_id).await.unwrap();
        let staging=pab_file_manager::StagingTarget::new(&root.path().join("staging"),&target).unwrap();let mut file=staging.open(true).unwrap();file.write_all(bytes).unwrap();file.sync_all().unwrap();staging.publish_open(&file,false).unwrap();
        if published_fact {service.store.confirm_transfer_publication(request.request_id).await.unwrap();}
        let pending=service.lookup_transfer(transfer_tests::actor(),request.request_id).await.unwrap();assert_eq!(pending.state,"committing");assert!(pending.metadata_result.is_none());drop(file);
        let busy=service.filesystem_slots.clone().try_acquire_many_owned(8).unwrap();assert_eq!(service.lookup_transfer(transfer_tests::actor(),request.request_id).await.unwrap(),pending);drop(busy);
        std::fs::write(&target,b"wrong").unwrap();assert_eq!(service.lookup_transfer(transfer_tests::actor(),request.request_id).await.unwrap(),pending);assert_eq!(std::fs::read(&target).unwrap(),b"wrong");
        std::fs::write(&target,bytes).unwrap();let before=std::fs::metadata(&target).unwrap().modified().unwrap();
        let mut changed_identity=service.clone();changed_identity.execution_context.identity.as_mut().unwrap().account_id.push_str("-new-account");assert_eq!(changed_identity.lookup_transfer(transfer_tests::actor(),request.request_id).await.unwrap(),pending);
        let foreign=pab_protocol::OperatorRef::account(pab_protocol::UserId::new(),transfer_tests::actor().endpoint_key());assert!(service.lookup_transfer(foreign,request.request_id).await.is_err());
        let recovered=service.lookup_transfer(transfer_tests::actor(),request.request_id).await.unwrap();assert_eq!(recovered.state,"completed");assert_eq!(recovered.published,Some(true));let result=recovered.metadata_result.as_ref().unwrap();result.validate_for(&metadata).unwrap();assert!(result.warnings.iter().any(|warning|warning.attribute==FileTransferAttribute::ModifiedTime));
        assert_eq!(std::fs::metadata(&target).unwrap().modified().unwrap(),before);assert_eq!(std::fs::read(&target).unwrap(),bytes);
        let reopened=transfer_tests::service(root.path()).await;assert_eq!(reopened.lookup_transfer(transfer_tests::actor(),request.request_id).await.unwrap(),recovered);
        std::fs::remove_file(&target).unwrap();assert_eq!(reopened.lookup_transfer(transfer_tests::actor(),request.request_id).await.unwrap(),recovered,"a historical completed receipt does not reopen an externally deleted file");
    }
}

#[tokio::test]
async fn resuming_cannot_change_original_attributes_or_source_preconditions() {
    use pab_protocol::{FileTransferMetadata,FileTransferSourceConstraint,FileByteCount};
    let root=tempfile::tempdir().unwrap();let service=transfer_tests::service(root.path()).await;
    let target=root.path().join("original.bin");std::fs::write(&target,b"existing target").unwrap();let native=pab_file_manager::encode_path(&target);
    let original=FileTransferRequest { destination_parent:None,request_id:RequestId::new(),metadata:Some(FileTransferMetadata {modified_at_unix_ms:Some(123),unix_mode:Some(0o640)}),source_constraint:Some(FileTransferSourceConstraint {size:FileByteCount(4),modified_at_unix_ms:Some(123)}),expected_execution:None,native_path:Some(native.clone()),execution:Default::default(),resume_from:None,operation:FileTransferOperation::Upload {path:native.display_label(),size:4,sha256:"0".repeat(64),overwrite:true}};
    service.store.accept_transfer(transfer_tests::actor(),&original,&service.execution_context).await.unwrap();
    service.store.finish_transfer(original.request_id,"failed",Some("fixture interrupted before IO")).await.unwrap();
    let before=service.store.get_transfer(transfer_tests::actor(),original.request_id).await.unwrap();
    let (first,second,client,server)=transfer_tests::pair().await;
    for change in 0..4 {
        let mut request=original.clone();request.request_id=RequestId::new();request.resume_from=Some(original.request_id);
        match change {0=>request.metadata.as_mut().unwrap().unix_mode=Some(0o600),1=>request.source_constraint.as_mut().unwrap().modified_at_unix_ms=Some(124),2=>request.metadata=None,_=>request.destination_parent=Some(pab_protocol::FileTransferParentConstraint {identity:"other:parent".into()})}
        let executor=service.clone();let remote=server.clone();let running=tokio::spawn(async move {executor.handle_stream(transfer_tests::actor(),remote.accept_bi(TIMEOUT).await.unwrap(),TIMEOUT).await});
        let mut stream=client.open_bi(TIMEOUT).await.unwrap();stream.send_frame_json(&DeviceTaskRequest::TransferFile {schema_version:DEVICE_TASK_SCHEMA_VERSION,request:request.clone()},TIMEOUT).await.unwrap();
        assert!(matches!(stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap(),DeviceTaskResponse::Error {..}));assert!(running.await.unwrap().is_err());
        assert!(service.store.existing_transfer(transfer_tests::actor(),&request).await.unwrap().is_none());assert_eq!(std::fs::read(&target).unwrap(),b"existing target");
        assert_eq!(service.store.get_transfer(transfer_tests::actor(),original.request_id).await.unwrap(),before);
    }
    assert!(!std::fs::read_dir(root.path()).unwrap().any(|entry|entry.unwrap().file_name().to_string_lossy().contains(".part")));
    first.close().await;second.close().await;
}

#[tokio::test]
async fn broken_upload_keeps_only_synced_complete_frames_and_preserves_the_original_target() {
    let root=tempfile::tempdir().unwrap();let service=transfer_tests::service(root.path()).await;
    let target=root.path().join("protected.bin");std::fs::write(&target,b"original target").unwrap();
    let bytes=vec![37u8;196607];let hash=format!("{:x}",Sha256::digest(&bytes));
    let native=pab_file_manager::encode_path(&target);
    let request=FileTransferRequest { destination_parent:None,source_constraint:None,metadata:None,request_id:RequestId::new(),expected_execution:Some(pab_protocol::FileExecutionIdentity::from_context(&service.execution_context).unwrap()),native_path:Some(native.clone()),execution:Default::default(),resume_from:None,operation:FileTransferOperation::Upload {path:native.display_label(),size:bytes.len() as u64,sha256:hash.clone(),overwrite:true}};
    let (first,second,client,server)=transfer_tests::pair().await;let executor=service.clone();
    let running=tokio::spawn(async move {executor.handle_stream(transfer_tests::actor(),server.accept_bi(TIMEOUT).await.unwrap(),TIMEOUT).await});
    let mut stream=client.open_bi(TIMEOUT).await.unwrap();
    stream.send_frame_json(&DeviceTaskRequest::TransferFile {schema_version:DEVICE_TASK_SCHEMA_VERSION,request:request.clone()},TIMEOUT).await.unwrap();
    assert!(matches!(stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap(),DeviceTaskResponse::TransferAccepted {..}));
    assert!(matches!(stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap(),DeviceTaskResponse::FileReady {offset:0,..}));
    stream.send_binary_frame(&bytes[..65536],TIMEOUT).await.unwrap();
    assert!(matches!(stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap(),DeviceTaskResponse::FileProgress {offset:65536}));
    drop(stream);assert!(tokio::time::timeout(TIMEOUT,running).await.unwrap().unwrap().is_err());
    let snapshot=service.store.get_transfer(transfer_tests::actor(),request.request_id).await.unwrap();
    assert_eq!(snapshot.offset,65536);assert_eq!(snapshot.state,"failed");assert_eq!(snapshot.published,Some(false));
    assert_eq!(std::fs::read(&target).unwrap(),b"original target");
    let staging=pab_file_manager::publish::staging_path(&target,&format!("pab-{}",&hash[..16])).unwrap();
    assert_eq!(std::fs::read(staging).unwrap(),bytes[..65536]);
    first.close().await;second.close().await;
}

#[test]
fn transfer_futures_fit_normal_windows_thread_stacks() {
    fn size<F:std::future::Future>(_:impl FnOnce()->F)->usize {std::mem::size_of::<F>()}
    fn service()->&'static TaskService {unreachable!()}
    fn stream()->PabBiStream {unreachable!()}
    fn stream_ref()->&'static mut PabBiStream {unreachable!()}
    fn request()->&'static FileTransferRequest {unreachable!()}
    for bytes in [size(||service().handle_stream(transfer_tests::actor(),stream(),TIMEOUT)),size(||service().transfer_stream(transfer_tests::actor(),request(),stream_ref(),TIMEOUT)),size(||native_transfer_case("fixture".into())),size(||stop_transfer_case(pab_protocol::FileTransferIntent::Pause))] {
        assert!(bytes<=128*1024,"transfer future grew to {bytes} bytes; box large branches before nesting them");
    }
}

#[test]
fn native_user_file_futures_fit_normal_windows_thread_stacks() {
    fn size<F:std::future::Future>(_:impl FnOnce()->F)->usize {std::mem::size_of::<F>()}
    fn service()->&'static TaskService {unreachable!()}
    fn request()->pab_protocol::FileSystemRequest {unreachable!()}
    fn prepared()->pab_os_control::execution::PreparedUser {unreachable!()}
    fn context()->pab_protocol::ExecutionContext {unreachable!()}
    fn engine()->super::filesystem_engine::FileEngine {unreachable!()}
    let sizes=[
        ("acceptance fixture",size(super::transfer_user_tests::native_user_transfer_case)),
        ("native filesystem worker",size(||crate::user_worker::filesystem::execute(std::path::Path::new("fixture"),prepared(),request(),vec![],context(),service().file_coordinator(),watch::channel(false).1))),
        ("filesystem acceptance",size(||async {service().execute_filesystem(transfer_tests::actor(),&request(),&[]).await})),
        ("filesystem engine",size(||engine().execute(request(),vec![],Some(context()),watch::channel(false).1))),
        ("service fixture initialization",size(||transfer_tests::service(std::path::Path::new("fixture")))),
        ("transfer history",size(||service().lookup_transfer(transfer_tests::actor(),RequestId::new()))),
    ];
    for (label,bytes) in &sizes {eprintln!("NATIVE_FILE_FUTURE {label} {bytes}");}
    assert!(sizes[0].1<=64*1024,"privileged fixture nests large file operations inline: {} bytes",sizes[0].1);
    assert!(sizes[5].1<=32*1024,"transfer history nests the native helper inline: {} bytes",sizes[5].1);
    for (label,bytes) in sizes {assert!(bytes<=128*1024,"{label} future grew to {bytes} bytes; box large branches before nesting them");}
}

#[tokio::test]
async fn transfer_stop_intent_and_flushed_checkpoint_are_not_premature_final_states() {
    use pab_protocol::FileTransferIntent;
    let root=tempfile::tempdir().unwrap();let service=transfer_tests::service(root.path()).await;
    let target=root.path().join("target.bin");let native=pab_file_manager::encode_path(&target);
    let request=FileTransferRequest { destination_parent:None, source_constraint:None,metadata:None,expected_execution:None,request_id:RequestId::new(),native_path:Some(native.clone()),execution:Default::default(),resume_from:None,operation:FileTransferOperation::Upload {path:native.display_label(),size:100,sha256:"0".repeat(64),overwrite:false}};
    service.store.accept_transfer(transfer_tests::actor(),&request,&service.execution_context).await.unwrap();
    assert!(service.store.confirm_transfer_stopped(request.request_id,20,100).await.is_err());
    let requested=service.store.request_transfer_stop(transfer_tests::actor(),request.request_id,FileTransferIntent::Pause).await.unwrap();
    assert_eq!(requested.state,"running");assert_eq!(requested.finished_at_unix_ms,None);
    service.store.confirm_transfer_stopped(request.request_id,20,100).await.unwrap();
    let stopping=service.store.get_transfer(transfer_tests::actor(),request.request_id).await.unwrap();
    assert_eq!(stopping.state,"stopping");assert_eq!(stopping.finished_at_unix_ms,None);
    service.store.request_transfer_stop(transfer_tests::actor(),request.request_id,FileTransferIntent::Cancel).await.unwrap();
    service.store.finalize_transfer_stopped(request.request_id).await.unwrap();
    let stopped=service.store.get_transfer(transfer_tests::actor(),request.request_id).await.unwrap();
    assert_eq!(stopped.state,"cancelled");assert_eq!(stopped.offset,20);assert!(stopped.finished_at_unix_ms.is_some());
    assert_eq!(service.store.request_transfer_stop(transfer_tests::actor(),request.request_id,FileTransferIntent::Pause).await.unwrap(),stopped);
    assert!(service.store.request_transfer_stop(transfer_tests::actor(),request.request_id,FileTransferIntent::Run).await.is_err());
}

#[tokio::test]
async fn transfer_stop_waits_for_a_durable_checkpoint_and_resumes_with_a_new_attempt() {
    use pab_protocol::FileTransferIntent;
    for intent in [FileTransferIntent::Pause,FileTransferIntent::Cancel] {
        Box::pin(stop_transfer_case(intent)).await;
    }
}
async fn stop_transfer_case(intent:pab_protocol::FileTransferIntent) {
    use pab_protocol::FileTransferIntent;
    let root=tempfile::tempdir().unwrap();let service=transfer_tests::service(root.path()).await;
    let target=root.path().join("existing.bin");tokio::fs::write(&target,b"preserve original").await.unwrap();
    let bytes:Vec<u8>=(0..262_144).map(|n|(n%251)as u8).collect();let digest=format!("{:x}",Sha256::digest(&bytes));
    let native=pab_file_manager::encode_path(&target);
    let request=FileTransferRequest { destination_parent:None, source_constraint:None,metadata:None,expected_execution:None,request_id:RequestId::new(),native_path:Some(native.clone()),execution:Default::default(),resume_from:None,operation:FileTransferOperation::Upload {path:native.display_label(),size:bytes.len()as u64,sha256:digest.clone(),overwrite:true}};
    let (a,b,client,server)=transfer_tests::pair().await;
    let remote=server.clone();let worker=service.clone();
    let running=tokio::spawn(async move {worker.handle_stream(transfer_tests::actor(),remote.accept_bi(TIMEOUT).await.unwrap(),TIMEOUT).await});
    let mut stream=client.open_bi(TIMEOUT).await.unwrap();
    stream.send_frame_json(&DeviceTaskRequest::TransferFile {schema_version:DEVICE_TASK_SCHEMA_VERSION,request:request.clone()},TIMEOUT).await.unwrap();
    assert!(matches!(stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap(),DeviceTaskResponse::TransferAccepted {..}));
    assert!(matches!(stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap(),DeviceTaskResponse::FileReady {offset:0,..}));
    stream.send_binary_frame(&bytes[..65_536],TIMEOUT).await.unwrap();
    assert!(matches!(stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap(),DeviceTaskResponse::FileProgress {offset:65_536}));
    let foreign=pab_protocol::OperatorRef::account(pab_protocol::UserId::new(),transfer_tests::actor().endpoint_key());
    assert!(service.store.request_transfer_stop(foreign,request.request_id,intent).await.is_err());
    // Separate authenticated control stream; the data stream intentionally
    // sends nothing further, so a stop cannot rely on the next data frame.
    let remote=server.clone();let worker=service.clone();
    let stopping=tokio::spawn(async move {worker.handle_stream(transfer_tests::actor(),remote.accept_bi(TIMEOUT).await.unwrap(),TIMEOUT).await});
    let mut control=client.open_bi(TIMEOUT).await.unwrap();
    control.send_json(&DeviceTaskRequest::StopTransfer {schema_version:DEVICE_TASK_SCHEMA_VERSION,request_id:request.request_id,intent},TIMEOUT).await.unwrap();
    let DeviceTaskResponse::Transfer {snapshot:requested}=control.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap() else {panic!("missing stop request acknowledgement")};
    assert!(matches!(requested.state.as_str(),"running"|"stopping"|"paused"|"cancelled"));
    stopping.await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(2),running).await.expect("stalled read ignored the stop").unwrap().unwrap();
    let stopped=service.store.get_transfer(transfer_tests::actor(),request.request_id).await.unwrap();
    assert_eq!(stopped.state,if intent==FileTransferIntent::Pause{"paused"}else{"cancelled"});
    assert_eq!(stopped.offset,65_536);assert_eq!(stopped.published,Some(false));assert!(stopped.finished_at_unix_ms.is_some());
    assert_eq!(tokio::fs::read(&target).await.unwrap(),b"preserve original");
    assert!(stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.is_err(),"stop EOF must not introduce JSON into a binary stream");
    if intent==FileTransferIntent::Pause {
        let mut resumed=request.clone();resumed.request_id=RequestId::new();resumed.resume_from=Some(request.request_id);
        let remote=server.clone();let worker=service.clone();
        let running=tokio::spawn(async move {worker.handle_stream(transfer_tests::actor(),remote.accept_bi(TIMEOUT).await.unwrap(),TIMEOUT).await});
        let mut stream=client.open_bi(TIMEOUT).await.unwrap();
        stream.send_frame_json(&DeviceTaskRequest::TransferFile {schema_version:DEVICE_TASK_SCHEMA_VERSION,request:resumed.clone()},TIMEOUT).await.unwrap();
        assert!(matches!(stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap(),DeviceTaskResponse::TransferAccepted {..}));
        assert!(matches!(stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap(),DeviceTaskResponse::FileReady {offset:65_536,..}));
        for chunk in bytes[65_536..].chunks(65_536) {stream.send_binary_frame(chunk,TIMEOUT).await.unwrap();assert!(matches!(stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap(),DeviceTaskResponse::FileProgress {..}));}
        assert!(matches!(stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap(),DeviceTaskResponse::FileComplete {sha256,..} if sha256==digest));
        running.await.unwrap().unwrap();assert_eq!(tokio::fs::read(&target).await.unwrap(),bytes);
        assert_eq!(service.store.get_transfer(transfer_tests::actor(),request.request_id).await.unwrap(),stopped,"resume cannot rewrite the original attempt's fact");
        assert_eq!(service.store.get_transfer(transfer_tests::actor(),resumed.request_id).await.unwrap().state,"completed");
    }
    a.close().await;b.close().await;
}

fn raw_name() -> std::ffi::OsString {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[0xd800, 46, 98, 105, 110])
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(vec![0xff, b'.', b'b', b'i', b'n'])
    }
}

#[tokio::test]
async fn transfer_native_identity_survives_real_binary_upload_and_publication_reconciliation() {
    native_transfer_case(raw_name()).await;
    native_transfer_case(format!("{}.bin", "x".repeat(240)).into()).await;
}

async fn native_transfer_case(name: std::ffi::OsString) {
    let root = tempfile::tempdir().unwrap();
    let service = transfer_tests::service(root.path()).await;
    let path = root.path().join(name);
    let native = pab_file_manager::encode_path(&path);
    let bytes: Vec<u8> = (0..131_071).map(|n| (n % 251) as u8).collect();
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let request = FileTransferRequest { destination_parent:None, source_constraint:None,metadata:None,expected_execution:None,
        request_id: RequestId::new(),
        native_path: Some(native.clone()),
        execution: Default::default(),
        resume_from: None,
        operation: FileTransferOperation::Upload {
            path: native.display_label(),
            size: bytes.len() as u64,
            sha256: hash.clone(),
            overwrite: false,
        },
    };
    let (a, b, client, server) = transfer_tests::pair().await;
    let download_server = server.clone();
    let worker = service.clone();
    let run = tokio::spawn(async move {
        worker
            .handle_stream(
                transfer_tests::actor(),
                server.accept_bi(TIMEOUT).await.unwrap(),
                TIMEOUT,
            )
            .await
    });
    let mut stream = client.open_bi(TIMEOUT).await.unwrap();
    stream
        .send_frame_json(
            &DeviceTaskRequest::TransferFile {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request: request.clone(),
            },
            TIMEOUT,
        )
        .await
        .unwrap();
    assert!(matches!(
        stream
            .receive_json::<DeviceTaskResponse>(TIMEOUT)
            .await
            .unwrap(),
        DeviceTaskResponse::TransferAccepted { .. }
    ));
    assert!(matches!(
        stream
            .receive_json::<DeviceTaskResponse>(TIMEOUT)
            .await
            .unwrap(),
        DeviceTaskResponse::FileReady { offset: 0, .. }
    ));
    let mut sent = 0;
    for chunk in bytes.chunks(65536) {
        stream.send_binary_frame(chunk, TIMEOUT).await.unwrap();
        sent += chunk.len() as u64;
        assert!(
            matches!(stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap(), DeviceTaskResponse::FileProgress { offset } if offset == sent)
        );
    }
    let completion = stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap();
    if !matches!(&completion, DeviceTaskResponse::FileComplete { size, sha256 } if *size == sent && sha256 == &hash) {
        panic!("upload did not complete: {completion:?}; worker: {:?}", run.await);
    }
    run.await.unwrap().unwrap();
    assert_eq!(tokio::fs::read(&path).await.unwrap(), bytes);
    assert_eq!(
        service
            .store
            .transfer_request(transfer_tests::actor(), request.request_id)
            .await
            .unwrap()
            .native_path,
        Some(native.clone())
    );
    let download = FileTransferRequest { destination_parent:None, source_constraint:None,metadata:None,expected_execution:None,
        request_id: RequestId::new(),
        native_path: Some(native.clone()),
        execution: Default::default(),
        resume_from: None,
        operation: FileTransferOperation::Download {
            path: native.display_label(),
            offset: 0,
            expected_sha256: Some(hash.clone()),
        },
    };
    let worker = service.clone();
    let run = tokio::spawn(async move {
        worker
            .handle_stream(
                transfer_tests::actor(),
                download_server.accept_bi(TIMEOUT).await.unwrap(),
                TIMEOUT,
            )
            .await
    });
    let mut stream = client.open_bi(TIMEOUT).await.unwrap();
    stream
        .send_frame_json(
            &DeviceTaskRequest::TransferFile {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request: download,
            },
            TIMEOUT,
        )
        .await
        .unwrap();
    assert!(matches!(
        stream
            .receive_json::<DeviceTaskResponse>(TIMEOUT)
            .await
            .unwrap(),
        DeviceTaskResponse::TransferAccepted { .. }
    ));
    assert!(matches!(
        stream
            .receive_json::<DeviceTaskResponse>(TIMEOUT)
            .await
            .unwrap(),
        DeviceTaskResponse::FileReady { offset: 0, .. }
    ));
    let mut received = Vec::new();
    while received.len() < bytes.len() {
        received.extend(stream.receive_binary_frame(TIMEOUT).await.unwrap());
    }
    assert_eq!(received, bytes);
    assert!(
        matches!(stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap(), DeviceTaskResponse::FileComplete { size, sha256 } if size == bytes.len() as u64 && sha256 == hash)
    );
    run.await.unwrap().unwrap();
    a.close().await;
    b.close().await;

    // Confirm an accepted-but-unrecorded publication from the native identity,
    // never by interpreting its escaped display label as a filesystem path.
    let mut unknown = request.clone();
    unknown.request_id = RequestId::new();
    tokio::fs::remove_file(&path).await.unwrap();
    service
        .store
        .accept_transfer(
            transfer_tests::actor(),
            &unknown,
            &service.execution_context,
        )
        .await
        .unwrap();
    service
        .store
        .transfer_progress(unknown.request_id, bytes.len() as u64, bytes.len() as u64)
        .await
        .unwrap();
    service
        .store
        .begin_transfer_publication(unknown.request_id)
        .await
        .unwrap();
    tokio::fs::write(&path, &bytes).await.unwrap();
    let reconciled = service
        .store
        .get_transfer(transfer_tests::actor(), unknown.request_id)
        .await
        .unwrap();
    assert_eq!(reconciled.state, "completed");
    assert_eq!(reconciled.published, Some(true));
}
