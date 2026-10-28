use super::*;

#[tokio::test]
async fn unsupported_attributes_and_source_constraints_are_rejected_before_acceptance() {
    let root=tempfile::tempdir().unwrap();let staging=root.path().join("staging");std::fs::create_dir(&staging).unwrap();let service=FileService::open(&root.path().join("attributes.sqlite3"),&staging,[9;32]).await.unwrap();let actor=OperatorRef::guest(EndpointKey::new([7;32]));
    for attributes in [false,true] {
        let mut request=upload();request.native_path=Some(pab_protocol::NativeFilePath::Utf8(request.operation.path().into()));
        if attributes {request.metadata=Some(pab_protocol::FileTransferMetadata {modified_at_unix_ms:Some(1_700_000_000_123),unix_mode:Some(0o644)});}else{request.source_constraint=Some(pab_protocol::FileTransferSourceConstraint {size:pab_protocol::FileByteCount(100),modified_at_unix_ms:Some(1_700_000_000_123)});}
        request.validate().unwrap();
        let error=service.accept_transfer("unsupported",actor,None,context(),&request).await.err().expect("unimplemented attributes must not be silently accepted");assert!(error.to_string().contains("updated Android storage adapter"));
        assert!(service.transfer_snapshot("unsupported",request.request_id).await.unwrap().is_none());assert_eq!(std::fs::read_dir(&service.staging).unwrap().count(),0);
    }
}

#[tokio::test]
async fn native_transfer_storage_rejects_non_utf8_identity_before_acceptance() {
    let root = tempfile::tempdir().unwrap();
    let service = FileService::open(&root.path().join("native.sqlite3"), root.path(), [9; 32])
        .await
        .unwrap();
    let actor = OperatorRef::guest(EndpointKey::new([7; 32]));
    let mut request = upload();
    let native = pab_protocol::NativeFilePath::UnixBytes(b"/workspace/\xff.bin".to_vec());
    request.native_path = Some(native.clone());
    if let FileTransferOperation::Upload { path, .. } = &mut request.operation {
        *path = native.display_label();
    }
    assert!(
        service
            .accept_transfer("native", actor, None, context(), &request)
            .await
            .is_err()
    );
    assert!(
        service
            .transfer_snapshot("native", request.request_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn completed_sender_allows_an_explicit_verified_download_resume_but_never_upload_replay() {
    let directory = tempfile::tempdir().unwrap();
    let service = FileService::open(
        &directory.path().join("files.sqlite"),
        directory.path(),
        [8; 32],
    )
    .await
    .unwrap();
    let operator = OperatorRef::guest(EndpointKey::new([7; 32]));
    let original = FileTransferRequest { destination_parent:None, source_constraint:None,metadata:None,expected_execution:None,
        native_path: None,
        request_id: RequestId::new(),
        execution: ExecutionSelection::default(),
        resume_from: None,
        operation: FileTransferOperation::Download {
            path: "/workspace/test.bin".into(),
            offset: 0,
            expected_sha256: None,
        },
    };
    let (mut snapshot, _, lease) = service
        .accept_transfer("first", operator, None, context(), &original)
        .await
        .unwrap();
    snapshot.state = "completed".into();
    snapshot.size = 100;
    snapshot.offset = 100;
    snapshot.sha256 = Some("a".repeat(64));
    service.save_transfer("first", &snapshot).await.unwrap();
    drop(lease);
    let mut resume = FileTransferRequest { destination_parent:None, source_constraint:None,metadata:None,expected_execution:None,
        native_path: None,
        request_id: RequestId::new(),
        execution: ExecutionSelection::default(),
        resume_from: Some(original.request_id),
        operation: FileTransferOperation::Download {
            path: "/workspace/test.bin".into(),
            offset: 37,
            expected_sha256: Some("b".repeat(64)),
        },
    };
    assert!(
        service
            .accept_transfer("first", operator, None, context(), &resume)
            .await
            .is_err()
    );
    if let FileTransferOperation::Download {
        expected_sha256, ..
    } = &mut resume.operation
    {
        *expected_sha256 = Some("a".repeat(64));
    }
    assert!(
        service
            .accept_transfer("other", operator, None, context(), &resume)
            .await
            .is_err()
    );
    let (next, _, lease) = service
        .accept_transfer("first", operator, None, context(), &resume)
        .await
        .unwrap();
    assert_eq!(next.state, "running");
    // The new source snapshot is not observed yet; expected hash belongs to the
    // request, not to a prematurely reported current-source observation.
    assert!(next.sha256.is_none());
    let saved: String =
        sqlx::query_scalar("SELECT request FROM file_transfers WHERE owner=? AND id=?")
            .bind("first")
            .bind(resume.request_id.to_string())
            .fetch_one(&service.pool)
            .await
            .unwrap();
    assert_eq!(
        serde_json::from_str::<FileTransferRequest>(&saved).unwrap(),
        resume
    );
    drop(lease);
    let mut second = resume.clone();
    second.request_id = RequestId::new();
    assert!(
        service
            .accept_transfer("first", operator, None, context(), &second)
            .await
            .is_err()
    );
    let upload = upload();
    let (mut sent, _, lease) = service
        .accept_transfer("first", operator, None, context(), &upload)
        .await
        .unwrap();
    sent.state = "completed".into();
    sent.published = Some(false); // Even absent publication evidence cannot authorize replay.
    service.save_transfer("first", &sent).await.unwrap();
    drop(lease);
    let mut retry = upload.clone();
    retry.request_id = RequestId::new();
    retry.resume_from = Some(upload.request_id);
    assert!(
        service
            .accept_transfer("first", operator, None, context(), &retry)
            .await
            .is_err()
    );
}

fn context() -> ExecutionContext {
    crate::peer::context(&crate::peer::RuntimeOptions {
        database_file: String::new(),
        password_hash: String::new(),
        password_version: 1,
        android_release: "14".into(),
        android_sdk: 34,
        storage_staging_directory: None,
    })
    .unwrap()
}

fn upload() -> FileTransferRequest {
    FileTransferRequest { destination_parent:None, source_constraint:None,metadata:None,expected_execution:None,
        native_path: None,
        request_id: RequestId::new(),
        execution: ExecutionSelection::default(),
        resume_from: None,
        operation: FileTransferOperation::Upload {
            path: "/workspace/test.bin".into(),
            size: 100,
            sha256: "a".repeat(64),
            overwrite: false,
        },
    }
}

#[tokio::test]
async fn lost_stream_is_interrupted_but_publication_uncertainty_cannot_be_resumed() {
    let directory = tempfile::tempdir().unwrap();
    let service = FileService::open(
        &directory.path().join("files.sqlite"),
        directory.path(),
        [8; 32],
    )
    .await
    .unwrap();
    let operator = OperatorRef::guest(EndpointKey::new([7; 32]));
    let request = upload();
    let (snapshot, stage, lease) = service
        .accept_transfer("first", operator, None, context(), &request)
        .await
        .unwrap();
    assert_eq!(snapshot.state, "running");
    assert!(
        service
            .transfer_snapshot("other", request.request_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        service
            .transfer_snapshot("first", request.request_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "running"
    );
    drop(lease);
    assert_eq!(
        service
            .transfer_snapshot("first", request.request_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "interrupted"
    );
    let (_, same, replayed) = service
        .accept_transfer("first", operator, None, context(), &request)
        .await
        .unwrap();
    assert_eq!(same, stage);
    assert!(replayed.is_none());
    let mut uncertain = snapshot;
    uncertain.state = "publishing".into();
    uncertain.published = None;
    service.save_transfer("first", &uncertain).await.unwrap();
    assert_eq!(
        service
            .transfer_snapshot("first", request.request_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "unconfirmed"
    );
    let mut resume = upload();
    resume.resume_from = Some(request.request_id);
    assert!(
        service
            .accept_transfer("first", operator, None, context(), &resume)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn explicit_resume_binds_caller_path_hash_and_the_retained_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let service = FileService::open(
        &directory.path().join("files.sqlite"),
        directory.path(),
        [8; 32],
    )
    .await
    .unwrap();
    let operator = OperatorRef::guest(EndpointKey::new([7; 32]));
    let request = upload();
    let (_, stage, lease) = service
        .accept_transfer("first", operator, None, context(), &request)
        .await
        .unwrap();
    let source = service.staging.join(format!("{stage}.part"));
    std::fs::write(&source, [1, 2, 3]).unwrap();
    drop(lease);
    let mut resume = upload();
    resume.resume_from = Some(request.request_id);
    assert!(
        service
            .accept_transfer("other", operator, None, context(), &resume)
            .await
            .is_err()
    );
    let mut wrong = resume.clone();
    if let FileTransferOperation::Upload { sha256, .. } = &mut wrong.operation {
        *sha256 = "b".repeat(64);
    }
    assert!(
        service
            .accept_transfer("first", operator, None, context(), &wrong)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&source).unwrap(), [1, 2, 3]);
    let (_, next, lease) = service
        .accept_transfer("first", operator, None, context(), &resume)
        .await
        .unwrap();
    assert!(lease.is_some());
    assert_eq!(next, stage);
    assert!(source.exists());
    assert_eq!(
        std::fs::read(service.staging.join(format!("{next}.part"))).unwrap(),
        [1, 2, 3]
    );
    let mut second = resume.clone();
    second.request_id = RequestId::new();
    assert!(
        service
            .accept_transfer("first", operator, None, context(), &second)
            .await
            .is_err()
    );
}
