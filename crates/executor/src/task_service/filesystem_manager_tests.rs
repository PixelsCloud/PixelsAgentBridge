use super::*;
use pab_protocol::{
    FileManagerOperation as Op, FileManagerResult as ResultBody, FileSystemAction,
    FileSystemRequest, RequestId,
};

fn request(operation: Op) -> FileSystemRequest {
    FileSystemRequest {
        request_id: RequestId::new(),
        execution: pab_protocol::ExecutionSelection::Service {},
        path: "file-manager".into(),
        operation: FileSystemAction::Managed { expected_execution:None,operation },
        payload_size: 0,
        payload_sha256: None,
    }
}

#[tokio::test]
async fn directory_recovery_polling_is_single_flight_cooled_down_and_bounded() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let service=transfer_tests::service(&root).await;let id=RequestId::new();
    let gate=service.directory_recovery_gate(id).await.unwrap();assert!(service.directory_recovery_gate(id).await.is_none());drop(gate);
    assert!(service.directory_recovery_gate(id).await.is_none(),"a finished/aborted query must not immediately launch a worker for every poll");
    for _ in 1..256 {drop(service.directory_recovery_gate(RequestId::new()).await.unwrap());}
    assert_eq!(service.directory_recovery_gates.lock().await.len(),256);
    assert!(service.directory_recovery_gate(RequestId::new()).await.is_none());
    {
        let gates=service.directory_recovery_gates.lock().await;
        for gate in gates.values(){*gate.lock().await=std::time::Instant::now()-std::time::Duration::from_secs(61);}
    }
    drop(service.directory_recovery_gate(RequestId::new()).await.unwrap());assert_eq!(service.directory_recovery_gates.lock().await.len(),1);
    service.close_for_test().await;drop(service);temp.close().unwrap();assert!(!root.exists());
}

#[tokio::test]
async fn filesystem_creation_admission_preserves_the_original_operation_identity_and_atomicity() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let service=transfer_tests::service(&root).await;let actor=transfer_tests::actor();
    let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(root.join("executor.sqlite3"))).await.unwrap();
    let request=request(Op::PrepareDirectory {path:pab_file_manager::encode_path(&root.join("child")),expected_parent:"object1".into(),existing_object:None});
    let fingerprint=filesystem_io::digest(&serde_json::to_vec(&request).unwrap());
    service.store.accept_filesystem_with_context(actor,&request,&fingerprint,Some(&service.execution_context)).await.unwrap();
    let mut changed=service.execution_context.clone();changed.identity.as_mut().unwrap().home.push_str("-new-session");
    assert!(service.store.accept_filesystem_with_context(actor,&request,&fingerprint,Some(&changed)).await.unwrap().is_some());
    let saved=service.store.filesystem_execution(actor,request.request_id).await.unwrap().unwrap();assert_eq!(saved.request,request);assert_eq!(saved.context,Some(service.execution_context.clone()));
    assert_eq!(saved.creation_receipt_pending,cfg!(unix));
    let foreign=pab_protocol::OperatorRef::account(pab_protocol::UserId::new(),pab_protocol::EndpointKey::new([84;32]));
    assert!(matches!(service.store.filesystem_execution(foreign,request.request_id).await,Err(crate::task_store::TaskStoreError::NotFound)));
    let mut tampered=request.clone();tampered.path.push_str("-changed");
    sqlx::query("UPDATE filesystem_execution SET request_json=? WHERE request_id=?").bind(serde_json::to_string(&tampered).unwrap()).bind(request.request_id.to_string()).execute(&pool).await.unwrap();
    assert!(matches!(service.store.filesystem_execution(actor,request.request_id).await,Err(crate::task_store::TaskStoreError::RequestConflict)));
    sqlx::query("DELETE FROM filesystem_execution WHERE request_id=?").bind(request.request_id.to_string()).execute(&pool).await.unwrap();
    assert!(service.store.filesystem_execution(actor,request.request_id).await.unwrap().is_none(),"legacy requests without an original execution record must not be reconstructed from a replay");
    assert!(service.store.accept_filesystem_with_context(actor,&request,&fingerprint,Some(&changed)).await.unwrap().is_some());
    assert!(service.store.filesystem_execution(actor,request.request_id).await.unwrap().is_none());
    sqlx::query("CREATE TRIGGER reject_creation_admission BEFORE INSERT ON filesystem_execution BEGIN SELECT RAISE(ABORT,'injected admission failure'); END").execute(&pool).await.unwrap();
    let mut next=request.clone();next.request_id=RequestId::new();let hash=filesystem_io::digest(&serde_json::to_vec(&next).unwrap());
    assert!(service.store.accept_filesystem_with_context(actor,&next,&hash,Some(&service.execution_context)).await.is_err());
    let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM read_operations WHERE request_id=?").bind(next.request_id.to_string()).fetch_one(&pool).await.unwrap();assert_eq!(count,0);
    assert!(!root.join("child").exists());pool.close().await;service.close_for_test().await;drop(service);temp.close().unwrap();assert!(!root.exists());
}

#[tokio::test]
async fn directory_recovery_requires_the_exact_frozen_fact_and_cannot_release_before_commit() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let service=transfer_tests::service(&root).await;let actor=transfer_tests::actor();
    let request=request(Op::PrepareDirectory {path:pab_file_manager::encode_path(&root.join("child")),expected_parent:"object1".into(),existing_object:None});
    service.store.accept_filesystem_with_context(actor,&request,&filesystem_io::digest(&serde_json::to_vec(&request).unwrap()),Some(&service.execution_context)).await.unwrap();
    let mut reply=pab_protocol::FileSystemReply::pending(&request);reply.execution_context=Some(service.execution_context.clone());service.store.begin_file_publication(&reply).await.unwrap();
    reply.state="completed".into();reply.file_manager=Some(pab_protocol::FileManagerReply {schema_version:pab_protocol::FILE_MANAGER_SCHEMA_VERSION,request_id:request.request_id,execution_context:reply.execution_context.clone(),result:ResultBody::DirectoryPrepared {path:pab_file_manager::encode_path(&root.join("child")),identity:"object2".into(),created:true}});
    assert!(service.store.directory_creation_receipt(&reply,true).await.is_err());
    let mut wrong=reply.clone();wrong.execution_context.as_mut().unwrap().identity.as_mut().unwrap().home.push_str("-wrong");
    assert!(service.store.directory_creation_receipt(&wrong,false).await.is_err());
    wrong=reply.clone();let ResultBody::DirectoryPrepared {path,..}=&mut wrong.file_manager.as_mut().unwrap().result else{unreachable!()};*path=pab_file_manager::encode_path(&root.join("other"));
    assert!(service.store.directory_creation_receipt(&wrong,false).await.is_err());
    assert_eq!(service.store.get_filesystem(actor,request.request_id).await.unwrap().state,"unconfirmed");
    service.store.directory_creation_receipt(&reply,false).await.unwrap();service.store.directory_creation_receipt(&reply,false).await.unwrap();
    assert_eq!(service.store.get_filesystem(actor,request.request_id).await.unwrap(),reply);
    wrong=reply.clone();let ResultBody::DirectoryPrepared {identity,..}=&mut wrong.file_manager.as_mut().unwrap().result else{unreachable!()};*identity="otherObject".into();
    assert!(service.store.directory_creation_receipt(&wrong,false).await.is_err());assert!(service.store.directory_creation_receipt(&wrong,true).await.is_err());
    assert_eq!(service.store.filesystem_execution(actor,request.request_id).await.unwrap().unwrap().creation_receipt_pending,cfg!(unix));
    service.store.directory_creation_receipt(&reply,true).await.unwrap();service.store.directory_creation_receipt(&reply,true).await.unwrap();
    assert!(!service.store.filesystem_execution(actor,request.request_id).await.unwrap().unwrap().creation_receipt_pending);
    assert!(!root.join("child").exists(),"this storage-only test must not perform a filesystem mutation");service.close_for_test().await;drop(service);temp.close().unwrap();assert!(!root.exists());
}

#[tokio::test]
#[cfg(unix)]
async fn directory_query_keeps_uncertainty_for_a_changed_identity_legacy_admission_or_different_operation() {
    for fault in ["identity","legacy","operation"] {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let service=transfer_tests::service(&root).await;let actor=transfer_tests::actor();
        let parent=pab_file_manager::DirectoryObject::open(&pab_file_manager::encode_path(&root),false).unwrap().identity().to_owned();let path=pab_file_manager::encode_path(&root.join("child"));
        let request=request(Op::PrepareDirectory {path:path.clone(),expected_parent:parent.clone(),existing_object:None});
        service.store.accept_filesystem_with_context(actor,&request,&filesystem_io::digest(&serde_json::to_vec(&request).unwrap()),Some(&service.execution_context)).await.unwrap();
        let mut original=pab_protocol::FileSystemReply::pending(&request);original.execution_context=Some(service.execution_context.clone());service.store.begin_file_publication(&original).await.unwrap();pab_agent_core::ensure_data_parent(&service.file_manager_cache).unwrap();
        let actual=if fault=="operation" {pab_file_manager::encode_path(&root.join("other"))}else{path};
        drop(pab_file_manager::DirectoryObject::prepare_journaled(&service.file_manager_cache,request.request_id,&actual,&parent,None).unwrap());
        let mut query=service.clone();
        if fault=="identity" {query.execution_context.identity.as_mut().unwrap().home.push_str("-other-generation");}
        if fault=="legacy" {
            let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(root.join("executor.sqlite3"))).await.unwrap();
            sqlx::query("DELETE FROM filesystem_execution WHERE request_id=?").bind(request.request_id.to_string()).execute(&pool).await.unwrap();pool.close().await;
        }
        let observed=query.lookup_filesystem(actor,request.request_id).await.unwrap();assert_eq!(observed.state,"unconfirmed");assert!(observed.file_manager.is_none());
        assert_eq!(pab_file_manager::recover_directory_staging(&service.file_manager_cache).unwrap().retained,1);
        if fault=="operation" {assert!(!root.join("child").exists());}
        service.close_for_test().await;drop((query,service));temp.close().unwrap();assert!(!root.exists());
    }
}

#[tokio::test]
#[cfg(unix)]
async fn worker_directory_recovery_retries_lost_result_and_release_acknowledgements_without_mutating_targets() {
    use filesystem_engine::{FileEngine,FileCoordination};
    for fault in ["none","result-ack","release-before-record"] {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let service=transfer_tests::service(&root).await;let actor=transfer_tests::actor();
        let path=pab_file_manager::encode_path(&root.join("child"));let parent=pab_file_manager::DirectoryObject::open(&pab_file_manager::encode_path(&root),false).unwrap().identity().to_owned();
        let request=request(Op::PrepareDirectory {path:path.clone(),expected_parent:parent.clone(),existing_object:None});
        service.store.accept_filesystem_with_context(actor,&request,&filesystem_io::digest(&serde_json::to_vec(&request).unwrap()),Some(&service.execution_context)).await.unwrap();
        let mut expected=pab_protocol::FileSystemReply::pending(&request);expected.execution_context=Some(service.execution_context.clone());service.store.begin_file_publication(&expected).await.unwrap();pab_agent_core::ensure_data_parent(&service.file_manager_cache).unwrap();
        let object=pab_file_manager::DirectoryObject::prepare_journaled(&service.file_manager_cache,request.request_id,&path,&parent,None).unwrap();
        expected.state="completed".into();expected.file_manager=Some(pab_protocol::FileManagerReply {schema_version:pab_protocol::FILE_MANAGER_SCHEMA_VERSION,request_id:request.request_id,execution_context:expected.execution_context.clone(),result:ResultBody::DirectoryPrepared {path:path.clone(),identity:object.identity().into(),created:true}});drop(object);
        std::fs::rename(root.join("child"),root.join("original")).unwrap();std::fs::create_dir(root.join("child")).unwrap();std::fs::write(root.join("child/sentinel"),b"replacement").unwrap();
        for round in 0..if fault=="none" {1}else{2} {
            let original=service.store.get_filesystem(actor,request.request_id).await.unwrap();let (send,mut events)=tokio::sync::mpsc::channel(8);let mut engine=FileEngine::worker(send);
            engine.file_manager=service.managed_engine(actor).unwrap().file_manager;
            let request=request.clone();let work=tokio::spawn(async move{engine.reconcile_directory(&request,original).await});let coordinator=service.file_coordinator();
            while let Some(event)=events.recv().await {
                let FileCoordination::DirectoryReceipt {reply,released,answer}=event else{panic!("recovery must not request target locks, publication or other mutation events")};
                if round==0 && fault=="release-before-record" && released {drop(answer);continue;}
                coordinator.directory_receipt(&reply,released).await.unwrap();
                if round==0 && fault=="result-ack" && !released {drop(answer);}else{answer.send(Ok(())).unwrap();}
            }
            let result=work.await.unwrap();if round==0 && fault=="result-ack" {assert!(result.is_err());}else{assert_eq!(result.unwrap(),expected);}
            assert_eq!(service.store.get_filesystem(actor,expected.request_id).await.unwrap(),expected);
            assert_eq!(std::fs::read(root.join("child/sentinel")).unwrap(),b"replacement");
        }
        assert!(!service.store.filesystem_execution(actor,request.request_id).await.unwrap().unwrap().creation_receipt_pending);
        assert_eq!(pab_file_manager::recover_directory_staging(&service.file_manager_cache).unwrap().retained,0);
        service.close_for_test().await;drop(service);temp.close().unwrap();assert!(!root.exists());
    }
}

#[tokio::test]
#[cfg(unix)]
async fn directory_publication_query_recovers_the_original_fact_and_retires_a_lost_ack() {
    for recorded in [false,true] {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
        let parent=root.join("parent");std::fs::create_dir(&parent).unwrap();let path=pab_file_manager::encode_path(&parent.join("child"));
        let expected_parent=pab_file_manager::DirectoryObject::open(&pab_file_manager::encode_path(&parent),false).unwrap().identity().to_owned();
        let request=request(Op::PrepareDirectory {path:path.clone(),expected_parent:expected_parent.clone(),existing_object:None});
        let actor=transfer_tests::actor();let service=transfer_tests::service(&root).await;
        let fingerprint=filesystem_io::digest(&serde_json::to_vec(&request).unwrap());
        service.store.accept_filesystem_with_context(actor,&request,&fingerprint,Some(&service.execution_context)).await.unwrap();
        let mut expected=pab_protocol::FileSystemReply::pending(&request);expected.execution_context=Some(service.execution_context.clone());
        service.store.begin_file_publication(&expected).await.unwrap();
        pab_agent_core::ensure_data_parent(&service.file_manager_cache).unwrap();
        let object=pab_file_manager::DirectoryObject::prepare_journaled(&service.file_manager_cache,request.request_id,&path,&expected_parent,None).unwrap();
        expected.state="completed".into();
        expected.file_manager=Some(pab_protocol::FileManagerReply {schema_version:pab_protocol::FILE_MANAGER_SCHEMA_VERSION,request_id:request.request_id,execution_context:expected.execution_context.clone(),result:ResultBody::DirectoryPrepared {path:path.clone(),identity:object.identity().into(),created:true}});
        if recorded {service.store.finish_filesystem(&expected).await.unwrap();}
        drop(object); // Simulate exit after publication, or after DB commit but before worker ACK.
        assert_eq!(pab_file_manager::recover_directory_staging(&service.file_manager_cache).unwrap().retained,1);
        std::fs::rename(parent.join("child"),parent.join("original")).unwrap();std::fs::create_dir(parent.join("child")).unwrap();std::fs::write(parent.join("child/sentinel"),b"replacement").unwrap();
        let restarted=transfer_tests::service(&root).await;
        let foreign=pab_protocol::OperatorRef::account(pab_protocol::UserId::new(),pab_protocol::EndpointKey::new([83;32]));
        assert!(restarted.lookup_filesystem(foreign,request.request_id).await.is_err());
        let observed=restarted.lookup_filesystem(actor,request.request_id).await.unwrap();
        assert_eq!(observed,expected,"original directory publication must survive the result/ACK gap");
        assert_eq!(restarted.execute_filesystem(actor,&request,&[]).await.unwrap().0,expected);
        assert_eq!(std::fs::read(parent.join("child/sentinel")).unwrap(),b"replacement");
        assert_eq!(pab_file_manager::recover_directory_staging(&service.file_manager_cache).unwrap().retained,0,"a durable original fact must retire the lost ACK receipt");
        drop((service,restarted));temp.close().unwrap();assert!(!root.exists());
    }
}

#[tokio::test]
async fn prepared_directory_receipt_survives_restart_without_reopening_its_name() {
    for merge in [false,true] {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let parent=root.join("parent");std::fs::create_dir(&parent).unwrap();let child=parent.join("child");
        let expected_parent=pab_file_manager::DirectoryObject::open(&pab_file_manager::encode_path(&parent),false).unwrap().identity().to_owned();
        let existing_object=if merge {std::fs::create_dir(&child).unwrap();Some(pab_file_manager::DirectoryObject::open(&pab_file_manager::encode_path(&child),false).unwrap().identity().to_owned())}else{None};
        let request=request(Op::PrepareDirectory {path:pab_file_manager::encode_path(&child),expected_parent,existing_object});let actor=transfer_tests::actor();let service=transfer_tests::service(&root).await;
        let reply=service.execute_filesystem(actor,&request,&[]).await.unwrap().0;assert_eq!(reply.state,"completed","{:?}",reply.error);
        assert!(matches!(&reply.file_manager.as_ref().unwrap().result,ResultBody::DirectoryPrepared {created,..} if *created==!merge));
        std::fs::rename(&child,parent.join("original")).unwrap();std::fs::create_dir(&child).unwrap();std::fs::write(child.join("sentinel"),b"replacement").unwrap();
        let restarted=transfer_tests::service(&root).await;assert_eq!(restarted.execute_filesystem(actor,&request,&[]).await.unwrap().0,reply);assert_eq!(restarted.lookup_filesystem(actor,request.request_id).await.unwrap(),reply);
        let mut fresh=request.clone();fresh.request_id=RequestId::new();assert_eq!(restarted.execute_filesystem(actor,&fresh,&[]).await.unwrap().0.state,"failed");
        assert_eq!(std::fs::read(child.join("sentinel")).unwrap(),b"replacement");
    }
}

#[tokio::test]
async fn directory_metadata_is_recorded_before_release_and_original_requests_do_not_repeat_changes() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
    let path=root.join("directory");std::fs::create_dir(&path).unwrap();
    let service=transfer_tests::service(&root).await;let actor=transfer_tests::actor();
    let inspect=request(Op::InspectDirectoryObject {path:pab_file_manager::encode_path(&path)});
    let reply=service.execute_filesystem(actor,&inspect,&[]).await.unwrap().0;
    assert_eq!(reply.state,"completed","{:?}",reply.error);
    let ResultBody::DirectoryObject {identity,..}=reply.file_manager.unwrap().result else {panic!("missing directory identity");};
    let set=request(Op::SetDirectoryMetadata {path:pab_file_manager::encode_path(&path),expected_object:identity,
        metadata:pab_protocol::FileTransferMetadata {modified_at_unix_ms:Some(1_700_000_000_123),unix_mode:None}});
    let reply=service.execute_filesystem(actor,&set,&[]).await.unwrap().0;
    assert_eq!(reply.state,"completed","{:?}",reply.error);
    assert!(matches!(&reply.file_manager.as_ref().unwrap().result,ResultBody::DirectoryMetadata {metadata,..} if metadata.warnings.is_empty()));
    assert_eq!(service.lookup_filesystem(actor,set.request_id).await.unwrap(),reply);
    let different=pab_protocol::OperatorRef::account(pab_protocol::UserId::new(),pab_protocol::EndpointKey::new([82;32]));
    assert!(service.lookup_filesystem(different,set.request_id).await.is_err());
    std::fs::rename(&path,root.join("original")).unwrap();std::fs::create_dir(&path).unwrap();
    let before=pab_file_manager::modified_at_unix_ms(&std::fs::metadata(&path).unwrap());
    assert_eq!(service.execute_filesystem(actor,&set,&[]).await.unwrap().0,reply);
    let mut stale=set.clone();stale.request_id=RequestId::new();
    assert_eq!(service.execute_filesystem(actor,&stale,&[]).await.unwrap().0.state,"failed");
    assert_eq!(pab_file_manager::modified_at_unix_ms(&std::fs::metadata(path).unwrap()),before);
}

async fn settled(
    service: &TaskService,
    actor: pab_protocol::OperatorRef,
    mut reply: pab_protocol::FileSystemReply,
) -> pab_protocol::FileSystemReply {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while matches!(reply.state.as_str(), "running" | "cancel_requested") {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            reply = service
                .lookup_filesystem(actor, reply.request_id)
                .await
                .unwrap();
        }
        reply
    })
    .await
    .expect("filesystem worker did not settle")
}

#[tokio::test]
async fn pinned_manager_identity_rejects_mutations_before_acceptance_and_observes_accepted_work() {
    let dir=tempfile::tempdir().unwrap();
    let root=dir.path().join("files");tokio::fs::create_dir(&root).await.unwrap();
    let service=transfer_tests::service(dir.path()).await;let actor=transfer_tests::actor();
    let expected=pab_protocol::FileExecutionIdentity::from_context(&service.execution_context).unwrap();
    let mut create=request(Op::CreateDirectory {parent:pab_file_manager::encode_path(&root),name:pab_file_manager::encode_path(std::path::Path::new("child"))});
    let FileSystemAction::Managed {expected_execution,..}=&mut create.operation else {unreachable!()};
    *expected_execution=Some(expected.clone());assert_eq!(create.schema_version(),8);
    let mut wrong=create.clone();wrong.request_id=RequestId::new();
    let FileSystemAction::Managed {expected_execution:Some(identity),..}=&mut wrong.operation else {unreachable!()};
    identity.identity.home.push_str("-different-account");
    let error=service.execute_filesystem(actor,&wrong,&[]).await.unwrap_err();
    assert!(matches!(error,TaskServiceError::ExecutionContext(_)));assert!(!root.join("child").exists());
    assert!(service.lookup_filesystem(actor,wrong.request_id).await.is_err());
    let reply=service.execute_filesystem(actor,&create,&[]).await.unwrap().0;assert_eq!(reply.state,"completed");
    tokio::fs::write(root.join("child").join("keep"),b"original").await.unwrap();
    let mut changed=service.clone();changed.execution_context.identity.as_mut().unwrap().home.push_str("-new-session");
    let replay=changed.execute_filesystem(actor,&create,&[]).await.unwrap().0;
    assert_eq!(replay,reply);assert_eq!(tokio::fs::read(root.join("child").join("keep")).await.unwrap(),b"original");
}

#[tokio::test]
async fn manager_index_uses_authenticated_actor_scope_and_replays_only_the_original_read() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("files");
    tokio::fs::create_dir(&root).await.unwrap();
    tokio::fs::write(root.join("中文.txt"), b"abc")
        .await
        .unwrap();
    let service = transfer_tests::service(dir.path()).await;
    let actor = transfer_tests::actor();
    let open = request(Op::OpenDirectory {
        path: pab_file_manager::encode_path(&root),
    });
    assert_eq!(open.schema_version(), 7);
    let accepted = service
        .execute_filesystem(actor, &open, &[])
        .await
        .unwrap()
        .0;
    assert_eq!(accepted.state, "running");
    assert!(accepted.supports_cancel());
    let opened = settled(&service, actor, accepted).await;
    assert_eq!(opened.state, "completed", "{:?}", opened.error);
    let ResultBody::Opened { snapshot_id } = opened.file_manager.as_ref().unwrap().result else {
        panic!("missing manager snapshot");
    };
    let replay = service
        .execute_filesystem(actor, &open, &[])
        .await
        .unwrap()
        .0;
    assert_eq!(replay.file_manager, opened.file_manager);
    let read = request(Op::ReadDirectory {
        snapshot_id,
        after: None,
        limit: 256,
        filter: Default::default(),
    });
    let page = service
        .execute_filesystem(actor, &read, &[])
        .await
        .unwrap()
        .0;
    assert_eq!(page.state, "completed", "{:?}", page.error);
    let ResultBody::Directory { page } = page.file_manager.unwrap().result else {
        panic!("missing directory page");
    };
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].name, "中文.txt");
    assert_eq!(page.entries[0].size.unwrap().0, 3);
    let other = pab_protocol::OperatorRef::account(
        pab_protocol::UserId::from_u128(999),
        pab_protocol::EndpointKey::new([9; 32]),
    );
    let foreign = request(Op::ReadDirectory {
        snapshot_id,
        after: None,
        limit: 2,
        filter: Default::default(),
    });
    let denied = service
        .execute_filesystem(other, &foreign, &[])
        .await
        .unwrap()
        .0;
    assert_eq!(denied.state, "failed");
    assert!(denied.file_manager.is_none());
    let close = request(Op::CloseDirectory { snapshot_id });
    assert_eq!(
        service
            .execute_filesystem(other, &close, &[])
            .await
            .unwrap()
            .0
            .state,
        "completed"
    );
    let still_there = request(Op::ReadDirectory {
        snapshot_id,
        after: None,
        limit: 2,
        filter: Default::default(),
    });
    assert_eq!(
        service
            .execute_filesystem(actor, &still_there, &[])
            .await
            .unwrap()
            .0
            .state,
        "completed"
    );
    assert_eq!(
        service
            .execute_filesystem(actor, &request(Op::CloseDirectory { snapshot_id }), &[])
            .await
            .unwrap()
            .0
            .state,
        "completed"
    );
    let closed = request(Op::ReadDirectory {
        snapshot_id,
        after: None,
        limit: 2,
        filter: Default::default(),
    });
    assert_eq!(
        service
            .execute_filesystem(actor, &closed, &[])
            .await
            .unwrap()
            .0
            .state,
        "failed"
    );
}

#[tokio::test]
async fn manager_directory_cancellation_waits_for_worker_cleanup_and_preserves_owner() {
    use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    tokio::fs::create_dir(&source).await.unwrap();
    let service = transfer_tests::service(dir.path()).await;
    let index = pab_file_manager::DirectoryIndex::open(&service.file_manager_cache)
        .await
        .unwrap();
    assert!(service.file_manager_index.set(index).is_ok());
    let gate_pool =
        SqlitePool::connect_with(SqliteConnectOptions::new().filename(&service.file_manager_cache))
            .await
            .unwrap();
    let gate = gate_pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let actor = transfer_tests::actor();
    let open = request(Op::OpenDirectory {
        path: pab_file_manager::encode_path(&source),
    });
    let accepted = service
        .execute_filesystem(actor, &open, &[])
        .await
        .unwrap()
        .0;
    assert_eq!(accepted.state, "running");
    let other = pab_protocol::OperatorRef::account(
        pab_protocol::UserId::from_u128(898),
        pab_protocol::EndpointKey::new([8; 32]),
    );
    assert!(service.cancel_hash(other, open.request_id).await.is_err());
    let stopping = service.cancel_hash(actor, open.request_id).await.unwrap();
    assert_eq!(stopping.state, "cancel_requested");
    gate.rollback().await.unwrap();
    let stopped = settled(&service, actor, stopping).await;
    assert_eq!(stopped.state, "cancelled", "{:?}", stopped.error);
    assert!(stopped.file_manager.is_none());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fm_snapshots")
        .fetch_one(&gate_pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "cancelled scan left a usable or leaked snapshot");
    gate_pool.close().await;
}

#[tokio::test]
async fn manager_inspection_preserves_native_paths_and_zero_length_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.bin");
    tokio::fs::write(&path, b"").await.unwrap();
    let service = transfer_tests::service(dir.path()).await;
    let req = request(Op::Inspect {
        path: pab_file_manager::encode_path(&path),
    });
    let reply = service
        .execute_filesystem(transfer_tests::actor(), &req, &[])
        .await
        .unwrap()
        .0;
    assert_eq!(reply.state, "completed", "{:?}", reply.error);
    let ResultBody::Entry { entry } = reply.file_manager.unwrap().result else {
        panic!("missing metadata");
    };
    assert_eq!(entry.size.unwrap().0, 0);
    assert_eq!(pab_file_manager::decode_path(&entry.path).unwrap(), path);
}

#[tokio::test]
async fn non_indexed_operations_never_open_or_require_the_directory_cache() {
    let dir=tempfile::tempdir().unwrap();let actor=transfer_tests::actor();
    let service=transfer_tests::service(dir.path()).await;
    let cache_parent=dir.path().join("not-a-directory");std::fs::write(&cache_parent,b"untouched sentinel").unwrap();
    for operation in [Op::Locations {},Op::CreateDirectory {parent:pab_file_manager::encode_path(dir.path()),name:pab_file_manager::encode_path(std::path::Path::new("created"))},Op::Inspect {path:pab_file_manager::encode_path(&cache_parent)}] {
        let mut engine=service.managed_engine(actor).unwrap();engine.file_manager.as_mut().unwrap().cache=cache_parent.join("cache.sqlite3");
        let request=request(operation);
        service.store.accept_filesystem_with_context(actor,&request,"fixture",Some(&service.execution_context)).await.unwrap();
        let (_send,cancel)=tokio::sync::watch::channel(false);
        let (reply,_)=engine.execute(request,vec![],Some(service.execution_context.clone()),cancel).await;
        assert_eq!(reply.state,"completed","{reply:?}");
    }
    assert!(dir.path().join("created").is_dir());assert_eq!(std::fs::read(cache_parent).unwrap(),b"untouched sentinel");
    assert!(service.file_manager_index.get().is_none());
}

#[tokio::test]
async fn manager_mutations_preserve_conflicts_and_durable_replay_does_not_delete_new_data() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("files");
    tokio::fs::create_dir(&root).await.unwrap();
    let service = transfer_tests::service(dir.path()).await;
    let actor = transfer_tests::actor();
    let create = request(Op::CreateDirectory {
        parent: pab_file_manager::encode_path(&root),
        name: "new".into(),
    });
    let created = service
        .execute_filesystem(actor, &create, &[])
        .await
        .unwrap()
        .0;
    assert_eq!(created.state, "completed", "{:?}", created.error);
    let new_path = pab_file_manager::encode_path(&root.join("new"));
    let rename = request(Op::Rename {
        path: new_path,
        name: "renamed".into(),
    });
    let renamed = service
        .execute_filesystem(actor, &rename, &[])
        .await
        .unwrap()
        .0;
    assert_eq!(renamed.state, "completed", "{:?}", renamed.error);
    #[cfg(windows)]
    {
        let case_only = request(Op::Rename {
            path: pab_file_manager::encode_path(&root.join("renamed")),
            name: "RENAMED".into(),
        });
        let changed = service
            .execute_filesystem(actor, &case_only, &[])
            .await
            .unwrap()
            .0;
        assert_eq!(changed.state, "completed", "{:?}", changed.error);
        assert_eq!(
            std::fs::read_dir(&root)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .file_name(),
            "RENAMED"
        );
    }
    tokio::fs::write(root.join("occupied"), b"original")
        .await
        .unwrap();
    let occupied = request(Op::Rename {
        path: pab_file_manager::encode_path(&root.join("renamed")),
        name: "occupied".into(),
    });
    let conflict = service
        .execute_filesystem(actor, &occupied, &[])
        .await
        .unwrap()
        .0;
    assert_eq!(conflict.state, "failed");
    assert_eq!(conflict.error.unwrap().code, "already_exists");
    assert_eq!(
        tokio::fs::read(root.join("occupied")).await.unwrap(),
        b"original"
    );
    let delete = request(Op::Delete {
        path: pab_file_manager::encode_path(&root.join("renamed")),
        recursive: true,
    });
    let removed = service
        .execute_filesystem(actor, &delete, &[])
        .await
        .unwrap()
        .0;
    assert_eq!(removed.state, "completed", "{:?}", removed.error);
    // A response retry must never apply an old delete to replacement data.
    tokio::fs::create_dir(root.join("renamed")).await.unwrap();
    tokio::fs::write(root.join("renamed/keep"), b"new data")
        .await
        .unwrap();
    let replay = service
        .execute_filesystem(actor, &delete, &[])
        .await
        .unwrap()
        .0;
    assert_eq!(replay.file_manager, removed.file_manager);
    assert_eq!(
        tokio::fs::read(root.join("renamed/keep")).await.unwrap(),
        b"new data"
    );
}

#[tokio::test]
async fn manager_restart_keeps_unknown_mutation_outcomes_and_never_replays_them() {
    for publication_recorded in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("selected");
        tokio::fs::create_dir(&path).await.unwrap();
        let service = transfer_tests::service(dir.path()).await;
        let actor = transfer_tests::actor();
        let delete = request(Op::Delete {
            path: pab_file_manager::encode_path(&path),
            recursive: true,
        });
        let fingerprint = super::filesystem_io::digest(&serde_json::to_vec(&delete).unwrap());
        assert!(
            service
                .store
                .accept_filesystem(actor, &delete, &fingerprint)
                .await
                .unwrap()
                .is_none()
        );
        if publication_recorded {
            service
                .store
                .begin_file_publication(&pab_protocol::FileSystemReply::pending(&delete))
                .await
                .unwrap();
        }
        // Simulate a process stopping after the OS accepted deletion but before
        // its result became durable, followed by unrelated replacement data.
        tokio::fs::remove_dir(&path).await.unwrap();
        tokio::fs::create_dir(&path).await.unwrap();
        tokio::fs::write(path.join("keep"), b"replacement")
            .await
            .unwrap();
        drop(service);
        let reopened = transfer_tests::service(dir.path()).await;
        let original = reopened
            .execute_filesystem(actor, &delete, &[])
            .await
            .unwrap()
            .0;
        assert_eq!(original.state, "unconfirmed");
        assert_eq!(
            tokio::fs::read(path.join("keep")).await.unwrap(),
            b"replacement"
        );
    }
}
