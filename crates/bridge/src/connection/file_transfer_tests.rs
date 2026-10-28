use super::*;
use pab_protocol::*;
use pab_transport::{PabConnection, PabEndpoint, PabEndpointAddress, PabEndpointConfig};
const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn corrupt_download_discards_only_its_open_staging_file_and_preserves_the_destination() {
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("target.bin");std::fs::write(&path,b"keep original destination").unwrap();
    let unrelated=dir.path().join(".target.bin.pab-unrelated.part");std::fs::write(&unrelated,b"keep unrelated partial").unwrap();
    let (first,second,client,server)=pair().await;let device=client.device_ref();let id=RequestId::new();
    let peer=tokio::spawn(async move {
        let mut environment=server.accept_bi(TIMEOUT).await.unwrap();
        assert!(matches!(environment.receive_json::<DeviceTaskRequest>(TIMEOUT).await.unwrap(),DeviceTaskRequest::GetEnvironment {..}));
        let reply=serde_json::json!({"type":"environment","transfer_schema_version":null,"context":{"device_ref":device,"execution":context(),"source":"executor_verified","observed_at_unix_ms":1,"freshness":"current"}});
        environment.send_json(&serde_json::from_value::<DeviceTaskResponse>(reply).unwrap(),TIMEOUT).await.unwrap();
        let mut stream=server.accept_bi(TIMEOUT).await.unwrap();
        assert!(matches!(stream.receive_json::<DeviceTaskRequest>(TIMEOUT).await.unwrap(),DeviceTaskRequest::DownloadFile {request_id,..} if request_id==id));
        let hash=format!("{:x}",Sha256::digest(b"data"));
        stream.send_frame_json(&DeviceTaskResponse::FileReady {size:4,offset:0,sha256:hash.clone()},TIMEOUT).await.unwrap();
        stream.send_binary_frame(b"evil",TIMEOUT).await.unwrap();
        stream.send_json(&DeviceTaskResponse::FileComplete {size:4,sha256:hash},TIMEOUT).await.unwrap();
    });
    let control=TransferControl::default();let result=client.download_file_controlled(id,"/remote/source",&path,true,&control,|_,_|{}).await;
    peer.await.unwrap();assert!(result.unwrap_err().to_string().contains("checksum mismatch"));
    assert_ne!(control.local_publication(),Some(true));assert!(!partial_path(&path,id).exists());
    assert_eq!(std::fs::read(&path).unwrap(),b"keep original destination");assert_eq!(std::fs::read(&unrelated).unwrap(),b"keep unrelated partial");
    first.close().await;second.close().await;
}

#[tokio::test]
async fn download_recovery_never_adopts_or_confirms_bytes_in_a_replaced_parent() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let parent=root.join("receiver");std::fs::create_dir(&parent).unwrap();
    let identity=pab_file_manager::DirectoryObject::open(&pab_file_manager::encode_path(&parent),false).unwrap().identity().to_owned();
    let constraint=FileTransferParentConstraint {identity};let path=parent.join("result");let id=RequestId::new();let partial=partial_path(&path,id);
    let bytes=b"published bytes";let hash=format!("{:x}",Sha256::digest(bytes));let requested=FileTransferMetadata::default();
    std::fs::write(&path,bytes).unwrap();std::fs::write(&partial,b"durable-extra").unwrap();
    std::fs::rename(&parent,root.join("original")).unwrap();std::fs::create_dir(&parent).unwrap();
    std::fs::write(&path,bytes).unwrap();std::fs::write(&partial,b"durable-extra").unwrap();
    assert!(restore_download_checkpoint(id,&path,100,Some(7),Some(&constraint)).await.is_err());
    assert!(verify_download_publication(&path,bytes.len()as u64,&hash,Some(&constraint)).await.is_err());
    assert!(recover_download_metadata(&path,bytes.len()as u64,&hash,&requested,Some(&constraint)).await.is_err());
    assert_eq!(std::fs::read(&partial).unwrap(),b"durable-extra");assert_eq!(std::fs::read(&path).unwrap(),bytes);
    std::fs::rename(&parent,root.join("replacement")).unwrap();std::fs::rename(root.join("original"),&parent).unwrap();
    assert_eq!(restore_download_checkpoint(id,&path,100,Some(7),Some(&constraint)).await.unwrap(),Some(7));
    assert!(verify_download_publication(&path,bytes.len()as u64,&hash,Some(&constraint)).await.unwrap());
    assert!(recover_download_metadata(&path,bytes.len()as u64,&hash,&requested,Some(&constraint)).await.unwrap().is_some());
    assert_eq!(std::fs::read(root.join("replacement").join(partial.file_name().unwrap())).unwrap(),b"durable-extra");
}

#[tokio::test]
async fn local_publication_preserves_the_verified_handle_and_reports_attribute_write_failures() {
    use std::{io::Write,sync::{Arc,Mutex}};
    for fail_at in [None,Some(1),Some(2),Some(3)] {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let destination=root.join("final.bin");std::fs::write(&destination,b"original").unwrap();
        let target=pab_file_manager::StagingTarget::new(&root.join("staging.bin"),&destination).unwrap();let mut file=target.open(true).unwrap();file.write_all(b"verified contents").unwrap();file.sync_all().unwrap();
        let control=TransferControl::default();control.set_digest(&format!("{:x}",Sha256::digest(b"verified contents")));control.set_checkpoint(17);
        let recorded=Arc::new(Mutex::new(Vec::<pab_file_manager::AttemptEvidence>::new()));let observed=recorded.clone();
        control.set_publication_guard(Arc::new(move |evidence| {let observed=observed.clone();Box::pin(async move {
            let mut entries=observed.lock().unwrap();entries.push(evidence);
            if fail_at==Some(entries.len()) {return Err(BridgeError::FileTransfer("fixture durable write failed".into()));}Ok(())
        })}));
        let metadata=FileTransferMetadata {modified_at_unix_ms:Some(1_700_000_000_123),unix_mode:Some(0o640)};
        let result=publish_download(target,file,true,Some(metadata.clone()),&control).await;
        assert_eq!(result.is_ok(),fail_at.is_none());
        let entries=recorded.lock().unwrap();assert!(entries[0].local_publication_intent);assert!(!entries[0].local_published);assert_eq!(entries[0].local_metadata_requested,Some(metadata.clone()));
        if fail_at==Some(1) {
            assert_eq!(std::fs::read(&destination).unwrap(),b"original");assert_eq!(control.local_publication(),None);assert!(!control.committing());
        } else {
            assert_eq!(std::fs::read(&destination).unwrap(),b"verified contents");assert_eq!(control.local_publication(),Some(true));assert!(entries[1].local_published);assert!(entries[1].local_metadata_result.is_none());
            if fail_at==Some(2) {assert!(control.local_metadata_pending());assert!(control.local_metadata_result().is_none());}
            else {
                let actual=control.local_metadata_result().unwrap();actual.validate_for(&metadata).unwrap();assert_eq!(actual.actual.modified_at_unix_ms,metadata.modified_at_unix_ms);assert_eq!(entries[2].local_metadata_result,Some(actual.clone()));assert!(!control.local_metadata_pending());
                #[cfg(unix)] {assert_eq!(actual.actual.unix_mode,Some(0o640));assert!(actual.warnings.is_empty());}
                #[cfg(windows)] {assert_eq!(actual.actual.unix_mode,None);assert_eq!(actual.warnings.len(),1);assert_eq!(actual.warnings[0].code,"unsupported");}
            }
        }
    }
}

#[tokio::test]
async fn local_download_recovery_keeps_only_flushed_bytes_and_never_steals_a_live_writer() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let destination=root.join("destination");fs::write(&destination,b"original").await.unwrap();
    let id=RequestId::new();let partial=partial_path(&destination,id);fs::write(&partial,b"durable tail").await.unwrap();
    assert_eq!(restore_download_checkpoint(id,&destination,100,Some(7),None).await.unwrap(),Some(7));
    assert_eq!(fs::read(&partial).await.unwrap(),b"durable");assert_eq!(fs::read(&destination).await.unwrap(),b"original");
    let target=pab_file_manager::StagingTarget::new(&partial,&destination).unwrap();let writer=target.open(false).unwrap();
    assert!(restore_download_checkpoint(id,&destination,100,Some(7),None).await.is_err());drop(writer);drop(target);
    assert!(restore_download_checkpoint(id,&destination,100,Some(8),None).await.is_err());assert_eq!(fs::read(&partial).await.unwrap(),b"durable");
    assert_eq!(restore_download_checkpoint(RequestId::new(),&destination,100,None,None).await.unwrap(),None);
    assert_eq!(fs::read(&destination).await.unwrap(),b"original");
}

#[tokio::test]
async fn recovered_publication_requires_the_final_file_size_and_independent_hash() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let path=root.join("final");let content=b"published bytes";let digest=format!("{:x}",Sha256::digest(content));
    assert!(!verify_download_publication(&path,content.len() as u64,&digest,None).await.unwrap());
    fs::write(&path,content).await.unwrap();assert!(verify_download_publication(&path,content.len() as u64,&digest,None).await.unwrap());
    assert!(!verify_download_publication(&path,1,&digest,None).await.unwrap());
    fs::write(&path,b"different bytes").await.unwrap();assert!(!verify_download_publication(&path,content.len() as u64,&digest,None).await.unwrap());
    assert_eq!(fs::read(&path).await.unwrap(),b"different bytes");
}

#[tokio::test]
async fn interrupted_attribute_recovery_requires_the_writer_lock_and_hash_and_never_mutates() {
    use std::io::Write;
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let path=root.join("result");let bytes=b"verified contents";let digest=format!("{:x}",Sha256::digest(bytes));
    let target=pab_file_manager::StagingTarget::new(&root.join("staging"),&path).unwrap();let mut file=target.open(true).unwrap();file.write_all(bytes).unwrap();file.sync_all().unwrap();
    let requested=FileTransferMetadata {modified_at_unix_ms:Some(1_700_000_000_123),unix_mode:Some(0o640)};
    target.publish_open(&file,false).unwrap();
    assert!(recover_download_metadata(&path,bytes.len()as u64,&digest,&requested,None).await.is_err(),"a live original writer must retain ownership");
    drop(file);
    let before=pab_file_manager::file_transfer_metadata(&std::fs::metadata(&path).unwrap());
    assert_ne!(before.modified_at_unix_ms,requested.modified_at_unix_ms);
    let result=recover_download_metadata(&path,bytes.len()as u64,&digest,&requested,None).await.unwrap().unwrap();result.validate_for(&requested).unwrap();assert_eq!(result.actual,before);assert!(result.warnings.iter().any(|warning|warning.attribute==FileTransferAttribute::ModifiedTime && warning.code=="not_preserved"));
    assert_eq!(pab_file_manager::file_transfer_metadata(&std::fs::metadata(&path).unwrap()),before);assert_eq!(std::fs::read(&path).unwrap(),bytes);
    std::fs::write(&path,b"different content").unwrap();assert!(recover_download_metadata(&path,bytes.len()as u64,&digest,&requested,None).await.unwrap().is_none());
    assert!(recover_download_metadata(&root.join("missing"),bytes.len()as u64,&digest,&requested,None).await.unwrap().is_none());
}

fn context() -> ExecutionContext {
    serde_json::from_value(serde_json::json!({"os_family":"linux","os_name":"Linux","os_version":"test","architecture":"x86_64","execution_scope":"native","path_style":"posix","interpreter":null,"cwd":"/home/fixture","environment_revision":"user-v1:test","identity":{"mode":"user","account_id":"uid:23001","account_name":"fixture","home":"/home/fixture","primary_group":23001,"session_id":null,"logon_id":null,"environment_source":"native_account"}})).unwrap()
}

pub(crate) async fn pair() -> (
    PabEndpoint,
    PabEndpoint,
    AuthenticatedDeviceConnection,
    PabConnection,
) {
    let config = PabEndpointConfig::new(vec!["https://127.0.0.1:1".parse().unwrap()]).unwrap();
    let key = || {
        iroh_base::SecretKey::from_bytes(
            &Sha256::digest(RequestId::new().to_string().as_bytes()).into(),
        )
    };
    let a = PabEndpoint::bind(config.clone(), key()).await.unwrap();
    let b = PabEndpoint::bind(config, key()).await.unwrap();
    let mut watch = b.watch_address();
    tokio::time::timeout(TIMEOUT, async {
        while watch.borrow().direct_addresses.is_empty() {
            watch.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let direct = watch
        .borrow()
        .direct_addresses
        .iter()
        .map(|address| {
            std::net::SocketAddr::new(
                if address.is_ipv4() {
                    std::net::Ipv4Addr::LOCALHOST.into()
                } else {
                    std::net::Ipv6Addr::LOCALHOST.into()
                },
                address.port(),
            )
        })
        .collect();
    let server = b.clone();
    let accept = tokio::spawn(async move { server.accept().await.unwrap().unwrap() });
    let connection = a
        .connect(
            *b.id().as_bytes(),
            &PabEndpointAddress {
                relay_urls: vec![],
                direct_addresses: direct,
            },
            TIMEOUT,
        )
        .await
        .unwrap();
    let server = accept.await.unwrap();
    let client = AuthenticatedDeviceConnection {
        file_execution_selections:Default::default(),connection,
        device_ref: DeviceRef {
            tenant_id: TenantId::from_u128(1),
            device_id: DeviceId::from_u128(2),
        },
        operator: OperatorRef::account(UserId::from_u128(3), EndpointKey::new([3; 32])),
        password_version: 1,
        authenticated_at_unix_ms: 1,
        operation_timeout: TIMEOUT,
    };
    (a, b, client, server)
}

#[tokio::test]
async fn transfer_bridge_checks_identity_native_paths_and_verified_resume() {
    // A real QUIC peer exercises the Bridge wire client. Native user execution
    // is independently verified by the Executor's three-platform fixture.
    for scenario in [
        "upload",
        "service_upload",
        "resume_download",
        "native_upload",
        "native_download",
        "wrong_identity",
        "old_peer",
        "rejected",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("中文 payload.bin");
        let bytes = b"binary\0payload with checksum".to_vec();
        let original = RequestId::new();
        let id = RequestId::new();
        let mut options = FileTransferOptions { destination_parent:None, source_constraint:None,metadata:None,expected_execution:None,
            execution: ExecutionSelection::User {
                context_ref: ExecutionContextRef::new(),
            },
            resume_from: None,
        };
        let mut selected_context = context();
        if scenario == "service_upload" {
            options.execution = Default::default();
            let identity = selected_context.identity.as_mut().unwrap();
            identity.mode = ExecutionMode::Service;
            identity.environment_source = ExecutionEnvironmentSource::ServiceProcess;
        }
        if scenario == "resume_download" {
            options.resume_from = Some(original);
            fs::write(partial_path(&path, original), &bytes[..7])
                .await
                .unwrap();
        } else if scenario != "native_download" {
            fs::write(&path, &bytes).await.unwrap();
        }
        let (a, b, client, server) = pair().await;
        let device = client.device_ref;
        let actor = client.operator;
        let expected_options = options.clone();
        let expected = bytes.clone();
        let observed_context = selected_context.clone();
        let native = NativeFilePath::UnixBytes(b"/remote/\xff.bin".to_vec());
        let expected_native = scenario.starts_with("native_").then(|| native.clone());
        let job = tokio::spawn(async move {
            let mut env = server.accept_bi(TIMEOUT).await.unwrap();
            assert!(matches!(
                env.receive_json::<DeviceTaskRequest>(TIMEOUT)
                    .await
                    .unwrap(),
                DeviceTaskRequest::GetEnvironment { .. }
            ));
            let value = serde_json::json!({"type":"environment","transfer_schema_version":if scenario=="old_peer" {1}else if scenario.starts_with("native_") {3}else{2},"context":{"device_ref":device,"execution":context(),"source":"executor_verified","observed_at_unix_ms":1,"freshness":"current"}});
            env.send_json(
                &serde_json::from_value::<DeviceTaskResponse>(value).unwrap(),
                TIMEOUT,
            )
            .await
            .unwrap();
            if scenario == "old_peer" {
                assert!(server.accept_bi(Duration::from_millis(200)).await.is_err());
                return;
            }
            let mut stream = server.accept_bi(TIMEOUT).await.unwrap();
            let DeviceTaskRequest::TransferFile { request, .. } = stream
                .receive_json::<DeviceTaskRequest>(TIMEOUT)
                .await
                .unwrap()
            else {
                panic!("user transfer fell back to legacy");
            };
            assert_eq!(request.execution, expected_options.execution);
            assert_eq!(request.resume_from, expected_options.resume_from);
            assert_eq!(request.native_path, expected_native);
            request.validate().unwrap();
            let mut observed = observed_context;
            if scenario == "wrong_identity" {
                observed.identity.as_mut().unwrap().account_id = "uid:23002".into();
            }
            if scenario == "rejected" {
                stream
                    .send_json(
                        &DeviceTaskResponse::Error {
                            code: DeviceTaskErrorCode::InvalidRequest,
                            message: "context unavailable".into(),
                        },
                        TIMEOUT,
                    )
                    .await
                    .unwrap();
                return;
            }
            let digest = format!("{:x}", Sha256::digest(&expected));
            let snapshot = TransferSnapshot {
                metadata_result:None,
                initiating_user: None,
                request_id: id,
                initiated_by: actor,
                direction: request.operation.direction().into(),
                path: request.operation.path().into(),
                state: "running".into(),
                offset: 0,
                size: expected.len() as u64,
                sha256: Some(digest.clone()),
                finished_at_unix_ms: None,
                message: None,
                published: None,
                execution_context: Some(observed),
            };
            stream
                .send_frame_json(&DeviceTaskResponse::TransferAccepted { snapshot }, TIMEOUT)
                .await
                .unwrap();
            if scenario == "wrong_identity" {
                assert!(
                    stream
                        .receive_binary_frame(Duration::from_millis(300))
                        .await
                        .is_err()
                );
                return;
            }
            let offset = if scenario == "resume_download" { 7 } else { 0 };
            stream
                .send_frame_json(
                    &DeviceTaskResponse::FileReady {
                        size: expected.len() as u64,
                        offset,
                        sha256: digest.clone(),
                    },
                    TIMEOUT,
                )
                .await
                .unwrap();
            if scenario == "resume_download" {
                assert!(
                    matches!(request.operation,FileTransferOperation::Download {offset:7,expected_sha256:Some(ref hash),..} if hash==&digest)
                );
                stream
                    .send_binary_frame(&expected[7..], TIMEOUT)
                    .await
                    .unwrap();
            } else if scenario == "native_download" {
                stream.send_binary_frame(&expected, TIMEOUT).await.unwrap();
            } else {
                assert_eq!(
                    stream.receive_binary_frame(TIMEOUT).await.unwrap(),
                    expected
                );
                stream
                    .send_frame_json(
                        &DeviceTaskResponse::FileProgress {
                            offset: expected.len() as u64,
                        },
                        TIMEOUT,
                    )
                    .await
                    .unwrap();
            }
            stream
                .send_json(
                    &DeviceTaskResponse::FileComplete {
                        size: expected.len() as u64,
                        sha256: digest,
                    },
                    TIMEOUT,
                )
                .await
                .unwrap();
        });
        let control = TransferControl::default();
        control.expect_context(selected_context.clone());
        let result = if scenario == "resume_download" {
            client
                .download_file_with_options(
                    id,
                    "/remote/file",
                    &path,
                    false,
                    &options,
                    Some(&format!("{:x}", Sha256::digest(&bytes))),
                    &control,
                    |_, _| {},
                )
                .await
        } else if scenario == "native_upload" {
            client
                .upload_native_file_with_options(
                    id,
                    &path,
                    &native,
                    false,
                    &options,
                    &control,
                    |_, _| {},
                )
                .await
        } else if scenario == "native_download" {
            client
                .download_native_file_with_options(
                    id,
                    &native,
                    &path,
                    false,
                    &options,
                    None,
                    &control,
                    |_, _| {},
                )
                .await
        } else {
            client
                .upload_file_with_options(
                    id,
                    &path,
                    "/remote/file",
                    false,
                    &options,
                    &control,
                    |_, _| {},
                )
                .await
        };
        assert_eq!(
            result.is_ok(),
            matches!(
                scenario,
                "upload"
                    | "service_upload"
                    | "resume_download"
                    | "native_upload"
                    | "native_download"
            ),
            "{scenario}: {result:?}"
        );
        if result.is_ok() {
            assert_eq!(
                control.accepted().unwrap().execution_context,
                Some(selected_context)
            );
        }
        if matches!(scenario, "resume_download" | "native_download") {
            assert_eq!(fs::read(&path).await.unwrap(), bytes);
            assert!(!partial_path(&path, original).exists());
            assert!(!partial_path(&path, id).exists());
        }
        if matches!(scenario, "old_peer" | "rejected") {
            assert!(!control.remote_requested());
        }
        job.await.unwrap();
        a.close().await;
        b.close().await;
    }
}
