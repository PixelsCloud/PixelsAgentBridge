use super::*;
use crate::task_service::transfer_tests::{actor, service};
use std::collections::VecDeque;

#[derive(Default)]
struct MemoryStream {
    input: VecDeque<Vec<u8>>,
    terminal: bool,
    sent: Vec<u8>,
    responses: Vec<DeviceTaskResponse>,
}
impl TransferStream for MemoryStream {
    async fn response(
        &mut self,
        response: &DeviceTaskResponse,
        end: bool,
        _: Duration,
    ) -> Result<(), TaskServiceError> {
        self.terminal |= end;
        self.responses.push(response.clone());
        Ok(())
    }
    async fn receive_binary_frame(&mut self, _: Duration) -> Result<Vec<u8>, TaskServiceError> {
        self.input.pop_front().ok_or_else(|| {
            io::Error::new(io::ErrorKind::UnexpectedEof, "fixture disconnected").into()
        })
    }
    async fn send_binary_frame(
        &mut self,
        bytes: &[u8],
        _: Duration,
    ) -> Result<(), TaskServiceError> {
        assert!(!bytes.is_empty() && bytes.len() <= 64 * 1024);
        self.sent.extend_from_slice(bytes);
        Ok(())
    }
}

#[tokio::test]
async fn frozen_source_size_and_time_are_checked_before_streaming_any_bytes() {
    let dir=tempfile::tempdir().unwrap();let svc=service(dir.path()).await;let source=dir.path().join("source.bin");std::fs::write(&source,b"original").unwrap();
    let modified=pab_file_manager::modified_at_unix_ms(&std::fs::metadata(&source).unwrap()).unwrap();
    for (size,time,success) in [(8,modified,true),(9,modified,false),(8,modified+1000,false)] {
        let id=RequestId::new();svc.store.start_transfer(id,actor(),"send",source.to_str().unwrap(),0,None).await.unwrap();
        let mut stream=MemoryStream::default();
        let constraint=pab_protocol::FileTransferSourceConstraint {size:pab_protocol::FileByteCount(size),modified_at_unix_ms:Some(time)};
        let result=TransferEngine::local(&svc.store,id,&svc.upload_locks).with_source_constraint(Some(constraint)).download(&mut stream,Duration::from_secs(5),&source,0,None).await;
        assert_eq!(result.is_ok(),success);
        if success {assert_eq!(stream.sent,b"original");assert!(stream.responses.iter().any(|response|matches!(response,DeviceTaskResponse::FileComplete {..})));}
        else {assert!(stream.sent.is_empty());assert!(stream.responses.is_empty());assert_ne!(svc.store.get_transfer(actor(),id).await.unwrap().state,"completed");}
    }
    assert_eq!(std::fs::read(source).unwrap(),b"original");
}

#[tokio::test]
async fn corrupt_upload_discards_only_its_open_staging_file_and_preserves_the_destination() {
    let dir=tempfile::tempdir().unwrap();let svc=service(dir.path()).await;let destination=dir.path().join("target.bin");
    std::fs::write(&destination,b"keep original destination").unwrap();let unrelated=dir.path().join(".target.bin.pab-unrelated.part");std::fs::write(&unrelated,b"keep unrelated partial").unwrap();
    let hash=format!("{:x}",Sha256::digest(b"data"));let id=RequestId::new();
    svc.store.start_transfer(id,actor(),"receive",destination.to_str().unwrap(),4,Some(&hash)).await.unwrap();
    let mut stream=MemoryStream {input:[b"evil".to_vec()].into(),..Default::default()};
    let result=TransferEngine::local(&svc.store,id,&svc.upload_locks).upload(&mut stream,Duration::from_secs(5),&destination,4,&hash,true).await;
    assert!(matches!(result,Err(TaskServiceError::InvalidRequest("file checksum mismatch"))));
    assert!(stream.responses.iter().any(|value|matches!(value,DeviceTaskResponse::Error {message,..} if message=="file checksum mismatch")));
    assert!(!stream.responses.iter().any(|value|matches!(value,DeviceTaskResponse::FileComplete {..})));
    assert!(!partial_path(&destination,&hash,None).exists());assert_eq!(std::fs::read(&destination).unwrap(),b"keep original destination");assert_eq!(std::fs::read(&unrelated).unwrap(),b"keep unrelated partial");
    assert_ne!(svc.store.get_transfer(actor(),id).await.unwrap().state,"completed");
}

#[tokio::test]
async fn mismatched_upload_source_size_is_rejected_before_staging() {
    let dir=tempfile::tempdir().unwrap();let svc=service(dir.path()).await;let destination=dir.path().join("target.bin");std::fs::write(&destination,b"original").unwrap();let hash=format!("{:x}",Sha256::digest(b"data"));
    let mut stream=MemoryStream {input:[b"data".to_vec()].into(),..Default::default()};
    let constraint=pab_protocol::FileTransferSourceConstraint {size:pab_protocol::FileByteCount(5),modified_at_unix_ms:None};
    assert!(TransferEngine::local(&svc.store,RequestId::new(),&svc.upload_locks).with_source_constraint(Some(constraint)).upload(&mut stream,Duration::from_secs(5),&destination,4,&hash,true).await.is_err());
    assert_eq!(std::fs::read(&destination).unwrap(),b"original");assert!(!partial_path(&destination,&hash,None).exists());assert!(stream.responses.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn source_change_after_open_is_rejected_before_file_ready() {
    struct MutatingRecorder {inner:LocalRecorder,path:std::path::PathBuf}
    impl TransferRecorder for MutatingRecorder {
        async fn record(&self,event:TransferRecord)->io::Result<()> {
            if matches!(event,TransferRecord::Progress {..}) {std::fs::write(&self.path,b"changed longer source")?;}
            self.inner.record(event).await
        }
    }
    let dir=tempfile::tempdir().unwrap();let svc=service(dir.path()).await;let source=dir.path().join("source.bin");std::fs::write(&source,b"original").unwrap();let id=RequestId::new();svc.store.start_transfer(id,actor(),"send",source.to_str().unwrap(),0,None).await.unwrap();
    let (locks,_receive)=mpsc::channel(8);let recorder=MutatingRecorder {inner:LocalRecorder {store:svc.store.clone(),request_id:id},path:source.clone()};
    let mut stream=MemoryStream::default();
    assert!(TransferEngine::worker(recorder,locks,b"fixture").download(&mut stream,Duration::from_secs(5),&source,0,None).await.is_err());
    assert!(stream.responses.is_empty());assert!(stream.sent.is_empty());assert_ne!(svc.store.get_transfer(actor(),id).await.unwrap().state,"completed");
}

#[tokio::test]
async fn receiver_attributes_are_set_on_the_verified_file_and_persisted_before_completion() {
    use pab_protocol::{FileTransferMetadata,FileTransferRequest,FileTransferOperation};
    let dir=tempfile::tempdir().unwrap();let svc=service(dir.path()).await;let destination=dir.path().join("metadata.bin");tokio::fs::write(&destination,b"original").await.unwrap();
    let bytes:Vec<u8>=(0..131071).map(|n|(n%251)as u8).collect();let hash=format!("{:x}",Sha256::digest(&bytes));let native=pab_file_manager::encode_path(&destination);
    let metadata=FileTransferMetadata {modified_at_unix_ms:Some(1_700_000_000_123),unix_mode:Some(0o440)};
    let request=FileTransferRequest { destination_parent:None,source_constraint:None,metadata:Some(metadata.clone()),expected_execution:None,native_path:Some(native.clone()),request_id:RequestId::new(),execution:Default::default(),resume_from:None,operation:FileTransferOperation::Upload {path:native.display_label(),size:bytes.len()as u64,sha256:hash.clone(),overwrite:true}};
    svc.store.accept_transfer(actor(),&request,&svc.execution_context).await.unwrap();
    let mut stream=MemoryStream {input:bytes.chunks(65536).map(Vec::from).collect(),..Default::default()};
    TransferEngine::local(&svc.store,request.request_id,&svc.upload_locks).with_metadata(Some(metadata.clone())).upload(&mut stream,Duration::from_secs(5),&destination,bytes.len()as u64,&hash,true).await.unwrap();
    assert!(stream.terminal);assert_eq!(tokio::fs::read(&destination).await.unwrap(),bytes);
    let receipt=svc.store.get_transfer(actor(),request.request_id).await.unwrap();assert_eq!(receipt.state,"completed");assert_eq!(receipt.published,Some(true));
    let outcome=receipt.metadata_result.unwrap();outcome.validate_for(&metadata).unwrap();assert_eq!(outcome.actual.modified_at_unix_ms,metadata.modified_at_unix_ms);
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(outcome.actual.unix_mode,Some(0o440));assert!(outcome.warnings.is_empty());
        std::fs::set_permissions(&destination,std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[cfg(not(unix))] {assert_eq!(outcome.actual.unix_mode,None);assert_eq!(outcome.warnings.len(),1);assert_eq!(outcome.warnings[0].code,"unsupported");}
}

#[tokio::test]
async fn invalid_privileged_attributes_fail_before_opening_staging_or_replacing_the_target() {
    let dir=tempfile::tempdir().unwrap();let svc=service(dir.path()).await;let target=dir.path().join("original.bin");tokio::fs::write(&target,b"original").await.unwrap();let id=RequestId::new();let hash=format!("{:x}",Sha256::digest(b"data"));
    let mut stream=MemoryStream {input:[b"data".to_vec()].into(),..Default::default()};
    let result=TransferEngine::local(&svc.store,id,&svc.upload_locks).with_metadata(Some(pab_protocol::FileTransferMetadata {modified_at_unix_ms:None,unix_mode:Some(0o4755)})).upload(&mut stream,Duration::from_secs(5),&target,4,&hash,true).await;
    assert!(result.is_err());assert_eq!(tokio::fs::read(&target).await.unwrap(),b"original");assert!(!partial_path(&target,&hash,None).exists());
}

#[tokio::test]
async fn transfer_publication_requires_parent_ack_and_lost_completion_stays_reconcilable() {
    // Same engine as production, with the future user worker's bounded channels.
    // Drop before rename permission, before recording its fact, after durable
    // publication but before its ack, or before the completion record.
    for disconnect in [None, Some(0), Some(1), Some(2), Some(3)] {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(dir.path()).await;
        let destination = dir.path().join("中文 transfer.bin");
        tokio::fs::write(&destination, b"original").await.unwrap();
        let bytes: Vec<u8> = (0..131071).map(|n| (n % 251) as u8).collect();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let id = RequestId::new();
        svc.store
            .start_transfer(
                id,
                actor(),
                "receive",
                destination.to_str().unwrap(),
                bytes.len() as u64,
                Some(&hash),
            )
            .await
            .unwrap();
        let recorder = LocalRecorder {
            store: svc.store.clone(),
            request_id: id,
        };
        let mut coordinator = svc.file_coordinator();
        let (send_locks, mut locks) = mpsc::channel(8);
        let (send_records, mut records) = mpsc::channel(8);
        let engine =
            TransferEngine::worker(WorkerRecorder(send_records), send_locks, b"fixture-user");
        let path = destination.clone();
        let expected = bytes.clone();
        let mut worker = tokio::spawn(async move {
            let mut stream = MemoryStream {
                input: expected.chunks(65536).map(Vec::from).collect(),
                ..Default::default()
            };
            let result = engine
                .upload(
                    &mut stream,
                    Duration::from_secs(5),
                    path.to_str().unwrap(),
                    expected.len() as u64,
                    &hash,
                    true,
                )
                .await;
            (result, stream.terminal)
        });
        let outcome = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    value = &mut worker => break value.unwrap(),
                    Some(event) = locks.recv() => match event {
                        FileCoordination::Lock { id, key, answer } => { let _ = answer.send(coordinator.lock(id, key)); },
                        FileCoordination::Unlock { id } => { coordinator.unlock(id).unwrap(); },
                        _ => panic!("unexpected filesystem record"),
                    },
                    Some(TransferRecordRequest { event, answer }) = records.recv() => {
                        if matches!(event, TransferRecord::BeginPublication) {
                            assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"original");
                            assert!(svc.upload_locks.try_acquire(&destination).await.unwrap().is_none());
                            if disconnect == Some(0) { drop(answer); continue; }
                        }
                        if matches!(event, TransferRecord::Completed) {
                            assert_eq!(tokio::fs::read(&destination).await.unwrap(), bytes);
                            if disconnect == Some(1) { drop(answer); continue; }
                        }
                        if matches!(event,TransferRecord::Published) {
                            // Windows deliberately prevents another handle
                            // reading bytes while finalization retains its lock.
                            assert_eq!(tokio::fs::metadata(&destination).await.unwrap().len(),bytes.len() as u64);
                            assert!(svc.upload_locks.try_acquire(&destination).await.unwrap().is_none());
                            #[cfg(windows)]
                            assert!(matches!(pab_file_manager::StagingTarget::new(&destination.with_file_name(".reservation-racer"),&destination), Err(error) if error.kind()==std::io::ErrorKind::WouldBlock));
                            if disconnect==Some(2) {drop(answer);continue;}
                            if disconnect==Some(3) {recorder.record(event).await.unwrap();drop(answer);continue;}
                        }
                        let _ = answer.send(recorder.record(event).await);
                    }
                }
            }
        }).await.unwrap();
        assert_eq!(outcome.0.is_ok(), disconnect.is_none());
        assert_eq!(outcome.1, disconnect.is_none());
        drop(coordinator); // Parent keeps the path locked through worker exit.
        assert!(
            svc.upload_locks
                .try_acquire(&destination)
                .await
                .unwrap()
                .is_some()
        );
        let snapshot = svc.store.get_transfer(actor(), id).await.unwrap();
        if disconnect == Some(0) {
            assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"original");
            assert_eq!(snapshot.state, "running");
        } else {
            assert_eq!(tokio::fs::read(&destination).await.unwrap(),bytes);
            assert_eq!(snapshot.state, "completed");
            assert_eq!(snapshot.published, Some(true));
        }
    }
}

#[tokio::test]
async fn transfer_engine_rejects_empty_or_oversized_adapter_chunks_without_publication() {
    for input in [vec![], vec![1; MAX_BINARY_FRAME_BYTES + 1], vec![1; 4]] {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(dir.path()).await;
        let path = dir.path().join("result.bin");
        tokio::fs::write(&path, b"original").await.unwrap();
        let id = RequestId::new();
        let hash = format!("{:x}", Sha256::digest([1; 3]));
        svc.store
            .start_transfer(
                id,
                actor(),
                "receive",
                path.to_str().unwrap(),
                3,
                Some(&hash),
            )
            .await
            .unwrap();
        let mut stream = MemoryStream {
            input: [input].into(),
            ..Default::default()
        };
        assert!(
            TransferEngine::local(&svc.store, id, &svc.upload_locks)
                .upload(
                    &mut stream,
                    Duration::from_secs(5),
                    path.to_str().unwrap(),
                    3,
                    &hash,
                    true
                )
                .await
                .is_err()
        );
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"original");
        assert!(stream.terminal);
        assert_eq!(svc.store.get_transfer(actor(), id).await.unwrap().offset, 0);
    }
}

#[tokio::test]
async fn transfer_engine_streams_resumed_download_and_records_the_whole_hash() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let path = dir.path().join("input.bin");
    let bytes: Vec<u8> = (0..196607).map(|n| (n % 251) as u8).collect();
    tokio::fs::write(&path, &bytes).await.unwrap();
    let id = RequestId::new();
    svc.store
        .start_transfer(id, actor(), "send", path.to_str().unwrap(), 0, None)
        .await
        .unwrap();
    let mut stream = MemoryStream::default();
    TransferEngine::local(&svc.store, id, &svc.upload_locks)
        .download(
            &mut stream,
            Duration::from_secs(5),
            path.to_str().unwrap(),
            65536,
            None,
        )
        .await
        .unwrap();
    assert_eq!(stream.sent, bytes[65536..]);
    assert!(stream.terminal);
    let snapshot = svc.store.get_transfer(actor(), id).await.unwrap();
    assert_eq!(snapshot.state, "completed");
    assert_eq!(
        snapshot.sha256,
        Some(format!("{:x}", Sha256::digest(&bytes)))
    );
    assert_eq!(snapshot.offset, bytes.len() as u64);
}
