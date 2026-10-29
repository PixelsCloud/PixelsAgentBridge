use super::batch::*;
use super::*;
use pab_protocol::*;

#[cfg(unix)]
#[tokio::test]
async fn private_receiving_ownership_is_persisted_immutable_and_bound_to_owner_target_and_parent() {
    use std::io::Write;
    let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
    let database=root.join("ownership.sqlite3");let store=BatchStore::open(&database).await.unwrap();
    let mut batch=spec();batch.direction=FileTransferDirection::Download;batch.destination=encode_path(&root);
    let mut source=item(0);source.source=encode_path(&root.join("source"));source.destination=encode_path(&root.join("destination"));batch.sources=vec![source.source.clone()];
    let parent=crate::DirectoryObject::open(&batch.destination,false).unwrap();
    let ready=crate::DirectoryReady {path:batch.destination.clone(),identity:parent.identity().into()};drop(parent);
    store.create_with_destination("owner",&batch,&ready).await.unwrap();store.append("owner",batch.id,&[source.clone()]).await.unwrap();store.finish_scan("owner",batch.id,None).await.unwrap();
    let worker=RequestId::new();store.heartbeat("owner",worker).await.unwrap();let entry=store.claim("owner",batch.id,worker).await.unwrap().unwrap();let attempt=entry.attempt.unwrap();
    store.save_item_target("owner",source.id,worker,&crate::ItemTarget {path:source.destination.clone(),overwrite:false,choice:FileConflictPolicy::Ask}).await.unwrap();
    let (stage,mut file)=crate::PrivateFileStaging::create(&decode_path(&source.destination).unwrap(),false,Some(&ready.identity)).unwrap();
    let evidence=AttemptEvidence {local_staging:Some(stage.receipt().clone()),..Default::default()};
    assert!(store.observe_attempt("foreign",source.id,worker,attempt,&evidence).await.is_err());
    assert!(store.observe_attempt("owner",source.id,RequestId::new(),attempt,&evidence).await.is_err());
    store.observe_attempt("owner",source.id,worker,attempt,&evidence).await.unwrap();
    // Only now does this receiving attempt write actual data.
    file.write_all(b"owned partial").unwrap();file.sync_all().unwrap();
    let reopened=BatchStore::open(&database).await.unwrap();
    assert_eq!(reopened.attempt_evidence("owner",attempt).await.unwrap().local_staging,evidence.local_staging);
    for field in 0..4 {
        let mut changed=evidence.clone();let receipt=changed.local_staging.as_mut().unwrap();
        match field {0=>receipt.directory_name=format!(".pab-receive-{}",RequestId::new()),1=>receipt.parent_identity="u:0:0:unknown".into(),2=>receipt.payload_identity="u:0:0:unknown".into(),_=>receipt.destination=encode_path(&root.join("foreign"))}
        assert!(reopened.observe_attempt("owner",source.id,worker,attempt,&changed).await.is_err());
    }
    reopened.observe_attempt("owner",source.id,worker,attempt,&evidence).await.unwrap();
    assert_eq!(std::fs::read(root.join(&stage.receipt().directory_name).join("payload")).unwrap(),b"owned partial");
    stage.discard_open(&file).unwrap();
    let discarded=AttemptEvidence {local_staging_discarded:true,..Default::default()};
    reopened.observe_attempt("owner",source.id,worker,attempt,&discarded).await.unwrap();
    reopened.observe_attempt("owner",source.id,worker,attempt,&discarded).await.unwrap();
    reopened.observe_attempt("owner",source.id,worker,attempt,&evidence).await.unwrap();
    assert!(reopened.attempt_evidence("owner",attempt).await.unwrap().local_staging_discarded,"delayed receiving evidence cannot revive bytes already removed");
    assert_eq!(file.metadata().unwrap().len(),13);
    assert!(!root.join(&stage.receipt().directory_name).join("payload").exists());
}

#[tokio::test]
async fn elapsed_checkpoints_are_idempotent_monotonic_and_survive_reopen_without_downtime() {
    let temp=tempfile::tempdir().unwrap();let path=temp.path().join("batch.db");
    let store=BatchStore::open(&path).await.unwrap();let batch=spec();let worker=RequestId::new();
    store.create("owner",&batch).await.unwrap();
    assert_eq!(store.snapshot("owner",batch.id).await.unwrap().active_elapsed_ms.0,0);
    store.heartbeat("owner",worker).await.unwrap();store.claim_runner("owner",batch.id,worker).await.unwrap();
    let token=store.begin_elapsed("owner",batch.id,worker).await.unwrap();
    for observed in [1_001,1_001,500,2_500] {store.checkpoint_elapsed("owner",batch.id,worker,token,FileByteCount(observed)).await.unwrap();}
    store.release_runner("owner",batch.id,worker).await.unwrap();
    assert!(store.checkpoint_elapsed("owner",batch.id,worker,token,FileByteCount(30_000)).await.is_err());
    let reopened=BatchStore::open(&path).await.unwrap();
    // Neither old wall-clock timestamps nor merely reading a paused/history
    // snapshot may invent elapsed time while the application was closed.
    sqlx::query("UPDATE fm_batches SET created=0,updated=0").execute(&reopened.pool).await.unwrap();
    for _ in 0..3 {assert_eq!(reopened.snapshot("owner",batch.id).await.unwrap().active_elapsed_ms.0,2_500);}
    let next=RequestId::new();reopened.heartbeat("owner",next).await.unwrap();reopened.claim_runner("owner",batch.id,next).await.unwrap();
    let resumed=reopened.begin_elapsed("owner",batch.id,next).await.unwrap();
    reopened.checkpoint_elapsed("owner",batch.id,next,resumed,FileByteCount(700)).await.unwrap();
    assert_eq!(reopened.snapshot("owner",batch.id).await.unwrap().active_elapsed_ms.0,3_200);
    let wire=serde_json::to_value(reopened.snapshot("owner",batch.id).await.unwrap()).unwrap();assert_eq!(wire["active_elapsed_ms"],"3200");
}

#[tokio::test]
async fn elapsed_fences_owner_worker_expired_lease_and_old_same_worker_round() {
    let temp=tempfile::tempdir().unwrap();let store=BatchStore::open(&temp.path().join("batch.db")).await.unwrap();let batch=spec();let worker=RequestId::new();let other=RequestId::new();
    store.create("owner",&batch).await.unwrap();store.heartbeat("owner",worker).await.unwrap();
    assert!(store.begin_elapsed("owner",batch.id,worker).await.is_err());
    store.claim_runner("owner",batch.id,worker).await.unwrap();let token=store.begin_elapsed("owner",batch.id,worker).await.unwrap();
    store.checkpoint_elapsed("owner",batch.id,worker,token,FileByteCount(17)).await.unwrap();
    assert!(store.begin_elapsed("foreign",batch.id,worker).await.is_err());
    assert!(store.checkpoint_elapsed("foreign",batch.id,worker,token,FileByteCount(99)).await.is_err());
    assert!(store.checkpoint_elapsed("owner",batch.id,other,token,FileByteCount(99)).await.is_err());
    let next=store.begin_elapsed("owner",batch.id,worker).await.unwrap();assert_ne!(token,next);
    assert!(store.checkpoint_elapsed("owner",batch.id,worker,token,FileByteCount(99)).await.is_err());
    store.checkpoint_elapsed("owner",batch.id,worker,next,FileByteCount(3)).await.unwrap();
    sqlx::query("UPDATE fm_workers SET heartbeat=0 WHERE id=?").bind(worker.to_string()).execute(&store.pool).await.unwrap();
    assert!(store.checkpoint_elapsed("owner",batch.id,worker,next,FileByteCount(9)).await.is_err());
    store.heartbeat("owner",other).await.unwrap();store.claim_runner("owner",batch.id,other).await.unwrap();
    assert!(store.checkpoint_elapsed("owner",batch.id,worker,next,FileByteCount(99)).await.is_err());
    let replacement=store.begin_elapsed("owner",batch.id,other).await.unwrap();store.checkpoint_elapsed("owner",batch.id,other,replacement,FileByteCount(5)).await.unwrap();
    assert_eq!(store.snapshot("owner",batch.id).await.unwrap().active_elapsed_ms.0,25);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM fm_elapsed").fetch_one(&store.pool).await.unwrap(),1);
}

#[tokio::test]
async fn elapsed_overflow_rolls_back_and_u64_precision_is_preserved() {
    let temp=tempfile::tempdir().unwrap();let store=BatchStore::open(&temp.path().join("batch.db")).await.unwrap();let batch=spec();let worker=RequestId::new();
    store.create("owner",&batch).await.unwrap();store.heartbeat("owner",worker).await.unwrap();store.claim_runner("owner",batch.id,worker).await.unwrap();
    let token=store.begin_elapsed("owner",batch.id,worker).await.unwrap();
    let large=(1u64<<53)+31;store.checkpoint_elapsed("owner",batch.id,worker,token,FileByteCount(large)).await.unwrap();
    assert_eq!(store.snapshot("owner",batch.id).await.unwrap().active_elapsed_ms.0,large);
    let next=store.begin_elapsed("owner",batch.id,worker).await.unwrap();
    assert!(matches!(store.checkpoint_elapsed("owner",batch.id,worker,next,FileByteCount(u64::MAX)).await,Err(FileManagerError::Capacity)));
    assert_eq!(store.snapshot("owner",batch.id).await.unwrap().active_elapsed_ms.0,large);
    store.checkpoint_elapsed("owner",batch.id,worker,next,FileByteCount(1)).await.unwrap();
    assert_eq!(store.snapshot("owner",batch.id).await.unwrap().active_elapsed_ms.0,large+1);
}

#[tokio::test]
async fn elapsed_old_schema_initializes_zero_and_concurrent_reports_cannot_double_charge() {
    let temp=tempfile::tempdir().unwrap();let path=temp.path().join("batch.db");let store=BatchStore::open(&path).await.unwrap();let batch=spec();let worker=RequestId::new();
    store.create("owner",&batch).await.unwrap();
    sqlx::query("DROP TABLE fm_elapsed").execute(&store.pool).await.unwrap();store.pool.close().await;
    let store=BatchStore::open(&path).await.unwrap();assert_eq!(store.snapshot("owner",batch.id).await.unwrap().active_elapsed_ms.0,0);
    store.heartbeat("owner",worker).await.unwrap();store.claim_runner("owner",batch.id,worker).await.unwrap();let token=store.begin_elapsed("owner",batch.id,worker).await.unwrap();
    let (a,b)=tokio::join!(store.checkpoint_elapsed("owner",batch.id,worker,token,FileByteCount(51)),store.checkpoint_elapsed("owner",batch.id,worker,token,FileByteCount(49)));a.unwrap();b.unwrap();
    assert_eq!(store.snapshot("owner",batch.id).await.unwrap().active_elapsed_ms.0,51);
}

fn spec() -> FileBatchSpec {
    FileBatchSpec {
        id: RequestId::new(),
        device: DeviceRef {
            tenant_id: TenantId::from_u128(1),
            device_id: DeviceId::from_u128(2),
        },
        code: "123456789".parse().unwrap(),
        direction: FileTransferDirection::Upload,
        sources: vec![NativeFilePath::Utf8("C:\\source".into())],
        destination: NativeFilePath::Utf8("D:\\destination".into()),
        execution: ExecutionSelection::Service {},
        conflict: FileConflictPolicy::Ask,
        force_relay: true,
    }
}
fn item(n: u64) -> FileBatchItemSpec {
    FileBatchItemSpec {
        id: RequestId::new(),
        source: NativeFilePath::Utf8(format!("C:\\source\\{n}")),
        destination: NativeFilePath::Utf8(format!("D:\\destination\\{n}")),
        relative_name: n.to_string(),
        kind: ManagedFileKind::File,
        size: FileByteCount(100),
        modified_at_unix_ms: None,
        unix_mode: None,
    }
}

#[tokio::test]
async fn conflict_scope_persists_only_for_the_selected_batch_and_scope() {
    for apply_remaining in [false, true] {
        for choice in [FileConflictPolicy::Replace, FileConflictPolicy::Skip, FileConflictPolicy::KeepBoth] {
            let temp = tempfile::tempdir().unwrap();
            let db = temp.path().join("conflicts.db");
            let store = BatchStore::open(&db).await.unwrap();
            let batch = spec(); let entries = [item(0), item(1)];
            store.create("owner", &batch).await.unwrap();
            store.append("owner", batch.id, &entries).await.unwrap();
            store.finish_scan("owner", batch.id, None).await.unwrap();
            let worker = RequestId::new();
            store.heartbeat("owner", worker).await.unwrap();
            let first = store.claim("owner", batch.id, worker).await.unwrap().unwrap();
            store.advance("owner", first.spec.id, worker, first.revision, FileItemState::WaitingConflict, FileByteCount(0), Some("Destination already exists")).await.unwrap();
            assert!(store.resolve_conflict_scope("foreign", batch.id, first.spec.id, choice, apply_remaining).await.is_err());
            store.resolve_conflict_scope("owner", batch.id, first.spec.id, choice, apply_remaining).await.unwrap();
            assert!(store.resolve_conflict_scope("owner", batch.id, first.spec.id, choice, apply_remaining).await.is_err());
            let reopened = BatchStore::open(&db).await.unwrap();
            assert_eq!(reopened.item_target("owner", first.spec.id, PathStyle::Windows).await.unwrap().choice, choice);
            assert_eq!(reopened.item_target("owner", entries[1].id, PathStyle::Windows).await.unwrap().choice, if apply_remaining { choice } else { FileConflictPolicy::Ask });
            let other = spec(); let other_item = item(9);
            reopened.create("owner", &other).await.unwrap();
            reopened.append("owner", other.id, &[other_item.clone()]).await.unwrap();
            assert_eq!(reopened.item_target("owner", other_item.id, PathStyle::Windows).await.unwrap().choice, FileConflictPolicy::Ask);
        }
    }
}

#[tokio::test]
async fn clear_completed_is_durable_owner_scoped_and_preserves_unfinished_tasks_and_receipts() {
    let temp = tempfile::tempdir().unwrap(); let db = temp.path().join("clear.db");
    let store = BatchStore::open(&db).await.unwrap();
    let mut cleared = Vec::new();
    for (owner, state, scan_done, message) in [
        ("owner", "completed", true, None), ("owner", "skipped", true, None),
        ("owner", "failed", true, None), ("owner", "cancelled", true, None),
        ("owner", "queued", true, None), ("owner", "paused", true, None),
        ("owner", "reconciling", true, None), ("owner", "completed", false, None),
        ("owner", "completed", true, Some("scan failed")), ("other", "completed", true, None),
    ] {
        let batch = spec(); let entry = item(0);
        store.create(owner, &batch).await.unwrap();
        store.append(owner, batch.id, &[entry.clone()]).await.unwrap();
        if scan_done { store.finish_scan(owner, batch.id, message).await.unwrap(); }
        sqlx::query("UPDATE fm_items SET state=? WHERE id=?").bind(state).bind(entry.id.to_string()).execute(&store.pool).await.unwrap();
        if owner == "owner" && scan_done && message.is_none() && ["completed", "skipped"].contains(&state) { cleared.push(batch.id.to_string()); }
    }
    let mut actual = store.clear_completed("owner").await.unwrap(); actual.sort(); cleared.sort();
    assert_eq!(actual, cleared);
    assert!(store.clear_completed("owner").await.unwrap().is_empty());
    let reopened = BatchStore::open(&db).await.unwrap();
    assert_eq!(reopened.list_visible("owner", 100).await.unwrap().len(), 7);
    assert_eq!(reopened.list_visible("other", 100).await.unwrap().len(), 1);
    assert_eq!(reopened.list("owner", 100).await.unwrap().len(), 9);
    for id in cleared {
        assert_eq!(reopened.items("owner", id.parse().unwrap(), 0, 128).await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn conflict_scope_requeues_waiting_and_late_conflicts_without_repeating_an_old_choice() {
    let temp = tempfile::tempdir().unwrap();
    let store = BatchStore::open(&temp.path().join("conflicts.db")).await.unwrap();
    let batch = spec(); let entries = [item(0), item(1), item(2)];
    store.create("owner", &batch).await.unwrap();
    store.append("owner", batch.id, &entries).await.unwrap();
    store.finish_scan("owner", batch.id, None).await.unwrap();
    let worker = RequestId::new(); store.heartbeat("owner", worker).await.unwrap();
    let first = store.claim("owner", batch.id, worker).await.unwrap().unwrap();
    store.advance("owner", first.spec.id, worker, first.revision, FileItemState::WaitingConflict, FileByteCount(0), Some("Destination already exists")).await.unwrap();
    let second = store.claim("owner", batch.id, worker).await.unwrap().unwrap();
    store.advance("owner", second.spec.id, worker, second.revision, FileItemState::WaitingConflict, FileByteCount(0), Some("Destination already exists")).await.unwrap();
    store.resolve_conflict("owner", batch.id, second.spec.id, FileConflictPolicy::Replace).await.unwrap();
    let second = store.claim("owner", batch.id, worker).await.unwrap().unwrap();
    store.advance("owner", second.spec.id, worker, second.revision, FileItemState::WaitingConflict, FileByteCount(0), Some("Different object types require skip or keep both")).await.unwrap();
    let late = store.claim("owner", batch.id, worker).await.unwrap().unwrap();
    store.resolve_conflict_scope("owner", batch.id, first.spec.id, FileConflictPolicy::Skip, true).await.unwrap();
    store.advance("owner", late.spec.id, worker, late.revision, FileItemState::WaitingConflict, FileByteCount(0), Some("Destination already exists")).await.unwrap();
    for entry in store.items("owner", batch.id, 0, 128).await.unwrap() {
        assert_eq!(entry.state, FileItemState::Queued);
        assert_eq!(store.item_target("owner", entry.spec.id, PathStyle::Windows).await.unwrap().choice, FileConflictPolicy::Skip);
    }
}

async fn ready_directory(store:&BatchStore,worker:RequestId,entry:&FileBatchItem) {
    let ready=DirectoryReady {path:entry.spec.destination.clone(),identity:format!("test:{}",entry.spec.id.to_string().replace('-',""))};
    store.save_item_target("owner",entry.spec.id,worker,&ItemTarget {path:ready.path.clone(),overwrite:false,choice:FileConflictPolicy::Ask}).await.unwrap();
    store.record_directory_ready("owner",entry.spec.id,worker,&ready).await.unwrap();
}

#[tokio::test]
async fn directory_preparation_freezes_original_request_and_commits_readiness_with_receipt() {
    for existing in [None,Some("volume:original".to_owned())] {
        for intent in [FileTransferIntent::Run,FileTransferIntent::Pause,FileTransferIntent::Cancel] {
            let temp=tempfile::tempdir().unwrap();let db=temp.path().join("batch.db");let store=BatchStore::open(&db).await.unwrap();let batch=spec();let entry=directory(0);let worker=RequestId::new();
            store.create_with_destination("owner",&batch,&DirectoryReady {path:batch.destination.clone(),identity:"volume:parent".into()}).await.unwrap();
            store.append("owner",batch.id,&[entry.clone()]).await.unwrap();store.finish_scan("owner",batch.id,None).await.unwrap();store.heartbeat("owner",worker).await.unwrap();
            let claimed=store.claim("owner",batch.id,worker).await.unwrap().unwrap();
            let target=ItemTarget {path:entry.destination.clone(),overwrite:false,choice:FileConflictPolicy::Ask};store.save_item_target("owner",entry.id,worker,&target).await.unwrap();
            assert!(store.begin_directory_preparation("foreign",entry.id,worker,&target.path,PathStyle::Windows,existing.clone()).await.is_err());
            assert!(store.begin_directory_preparation("owner",entry.id,worker,&NativeFilePath::Utf8("D:\\unrelated\\0".into()),PathStyle::Windows,existing.clone()).await.is_err());
            let preparation=store.begin_directory_preparation("owner",entry.id,worker,&target.path,PathStyle::Windows,existing.clone()).await.unwrap().unwrap();
            assert_eq!(Some(preparation.request_id),claimed.attempt);
            assert_eq!(preparation.operation,FileManagerOperation::PrepareDirectory {path:target.path.clone(),expected_parent:"volume:parent".into(),existing_object:existing.clone()});
            assert_eq!(store.item("owner",entry.id).await.unwrap().state,FileItemState::Publishing);
            assert!(store.begin_directory_preparation("owner",entry.id,worker,&target.path,PathStyle::Windows,existing.clone()).await.unwrap().is_none());
            store.set_intent("owner",batch.id,intent).await.unwrap();
            store.retire_worker("owner",batch.id,worker).await.unwrap();
            let reopened=BatchStore::open(&db).await.unwrap();let observer=RequestId::new();reopened.heartbeat("owner",observer).await.unwrap();
            let observing=reopened.claim_for_observation("owner",entry.id,observer).await.unwrap();assert_eq!(observing.attempt,claimed.attempt);
            let original=reopened.directory_preparation("owner",preparation.request_id).await.unwrap().unwrap();assert_eq!(original.operation,preparation.operation);assert!(original.result.is_none());
            assert!(reopened.directory_preparation("foreign",preparation.request_id).await.is_err());
            let result=FileManagerResult::DirectoryPrepared {path:target.path.clone(),identity:existing.clone().unwrap_or_else(||"volume:created".into()),created:existing.is_none()};
            let mut invalid=result.clone();if let FileManagerResult::DirectoryPrepared {created,..}=&mut invalid {*created=!*created;}
            assert!(reopened.record_directory_preparation("owner",entry.id,observer,preparation.request_id,&invalid).await.is_err());
            assert!(reopened.directory_ready("owner",entry.id).await.unwrap().is_none());assert!(reopened.directory_preparation("owner",preparation.request_id).await.unwrap().unwrap().result.is_none());
            reopened.record_directory_preparation("owner",entry.id,observer,preparation.request_id,&result).await.unwrap();
            let ready=reopened.directory_ready("owner",entry.id).await.unwrap().unwrap();assert_eq!(ready.path,target.path);
            assert_eq!(ready.identity,existing.clone().unwrap_or_else(||"volume:created".into()));
            assert_eq!(reopened.directory_preparation("owner",preparation.request_id).await.unwrap().unwrap().result,Some(result));
            assert_eq!(reopened.item("owner",entry.id).await.unwrap().state,match intent {FileTransferIntent::Run=>FileItemState::WaitingChildren,FileTransferIntent::Pause=>FileItemState::Paused,FileTransferIntent::Cancel=>FileItemState::Cancelled});
        }
    }
}

#[tokio::test]
async fn directory_preparation_stop_before_admission_and_unknown_result_cannot_retarget() {
    let temp=tempfile::tempdir().unwrap();let store=BatchStore::open(&temp.path().join("batch.db")).await.unwrap();let batch=spec();let entry=directory(0);let worker=RequestId::new();
    store.create_with_destination("owner",&batch,&DirectoryReady {path:batch.destination.clone(),identity:"volume:parent".into()}).await.unwrap();store.append("owner",batch.id,&[entry.clone()]).await.unwrap();store.finish_scan("owner",batch.id,None).await.unwrap();store.heartbeat("owner",worker).await.unwrap();
    let claimed=store.claim("owner",batch.id,worker).await.unwrap().unwrap();let target=ItemTarget {path:entry.destination.clone(),overwrite:false,choice:FileConflictPolicy::Ask};store.save_item_target("owner",entry.id,worker,&target).await.unwrap();
    store.set_intent("owner",batch.id,FileTransferIntent::Pause).await.unwrap();assert!(store.begin_directory_preparation("owner",entry.id,worker,&target.path,PathStyle::Windows,None).await.unwrap().is_none());assert!(store.directory_preparation("owner",claimed.attempt.unwrap()).await.unwrap().is_none());
    let stopped=store.item("owner",entry.id).await.unwrap();store.advance("owner",entry.id,worker,stopped.revision,FileItemState::Paused,FileByteCount(0),None).await.unwrap();store.set_intent("owner",batch.id,FileTransferIntent::Run).await.unwrap();
    let resumed=store.claim("owner",batch.id,worker).await.unwrap().unwrap();assert_ne!(resumed.attempt,claimed.attempt);
    let preparation=store.begin_directory_preparation("owner",entry.id,worker,&target.path,PathStyle::Windows,None).await.unwrap().unwrap();
    // Simulate a coordinator losing its phase update. An admitted mutation still
    // freezes the target even if another caller presents a preparing state.
    sqlx::query("UPDATE fm_items SET state='preparing' WHERE id=?").bind(entry.id.to_string()).execute(&store.pool).await.unwrap();
    let changed=ItemTarget {path:NativeFilePath::Utf8("D:\\destination\\changed".into()),..target.clone()};assert!(store.save_item_target("owner",entry.id,worker,&changed).await.is_err());
    assert_eq!(store.directory_preparation("owner",preparation.request_id).await.unwrap().unwrap().operation,preparation.operation);
    assert_eq!(store.item_target("owner",entry.id,PathStyle::Windows).await.unwrap().path,target.path);
}

#[tokio::test]
async fn reserved_submission_binds_once_after_reopen_and_never_upgrades_legacy_work() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().to_owned();let db=root.join("batch.db");
    let store=BatchStore::open(&db).await.unwrap();let batch=spec();
    assert!(store.reserve_submission("owner",&batch).await.unwrap());
    assert!(!store.reserve_submission("owner",&batch).await.unwrap());
    assert_eq!(store.snapshot("owner",batch.id).await.unwrap().spec,batch);
    let worker=RequestId::new();store.heartbeat("owner",worker).await.unwrap();
    assert!(store.claim_runner("owner",batch.id,worker).await.is_err());
    assert!(store.append("owner",batch.id,&[item(1)]).await.is_err());
    assert!(store.append_planned_batch("owner",batch.id,&[PlannedItem {spec:item(2),parent:None,skip:None}]).await.is_err());
    assert!(store.items("owner",batch.id,0,128).await.unwrap().is_empty());
    store.pool.close().await;drop(store);
    let store=BatchStore::open(&db).await.unwrap();
    let original=DirectoryReady {path:batch.destination.clone(),identity:"volume:original".into()};
    assert!(!store.create_with_destination("owner",&batch,&original).await.unwrap());
    assert!(!store.reserve_submission("owner",&batch).await.unwrap());
    assert!(!store.create_with_destination("owner",&batch,&original).await.unwrap());
    let replacement=DirectoryReady {identity:"volume:replacement".into(),..original.clone()};
    assert!(matches!(store.create_with_destination("owner",&batch,&replacement).await,Err(FileManagerError::Conflict)));
    assert!(matches!(store.create_with_destination("foreign",&batch,&original).await,Err(FileManagerError::NotFound)));
    let mut changed=batch.clone();changed.conflict=FileConflictPolicy::Replace;
    assert!(matches!(store.reserve_submission("owner",&changed).await,Err(FileManagerError::Conflict)));
    store.append("owner",batch.id,&[item(3)]).await.unwrap();
    assert_eq!(store.snapshot("owner",batch.id).await.unwrap().total.0,1);
    let legacy=spec();store.create("owner",&legacy).await.unwrap();
    assert!(!store.reserve_submission("owner",&legacy).await.unwrap(),"replay must not manufacture a reservation marker");
    let legacy_anchor=DirectoryReady {path:legacy.destination.clone(),identity:"volume:legacynow".into()};
    FileTransferParentConstraint {identity:legacy_anchor.identity.clone()}.validate().unwrap();
    let rejected=store.create_with_destination("owner",&legacy,&legacy_anchor).await;
    assert!(matches!(rejected,Err(FileManagerError::Conflict)),"{rejected:?}");
    store.pool.close().await;drop(store);temp.close().unwrap();assert!(!root.exists());
}

#[tokio::test]
async fn separate_preflight_writers_cannot_bind_a_reserved_batch_to_two_directories() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().to_owned();let db=root.join("batch.db");
    let first=BatchStore::open(&db).await.unwrap();let second=BatchStore::open(&db).await.unwrap();let batch=spec();
    first.reserve_submission("owner",&batch).await.unwrap();
    let a=DirectoryReady {path:batch.destination.clone(),identity:"volume:first".into()};
    let b=DirectoryReady {identity:"volume:second".into(),..a.clone()};
    let (left,right)=tokio::join!(first.create_with_destination("owner",&batch,&a),second.create_with_destination("owner",&batch,&b));
    assert_ne!(left.is_ok(),right.is_ok(),"exactly one first binding may commit");
    let (winner,loser)=if left.is_ok(){(&a,&b)}else{(&b,&a)};
    assert!(!first.create_with_destination("owner",&batch,winner).await.unwrap());
    assert!(matches!(second.create_with_destination("owner",&batch,loser).await,Err(FileManagerError::Conflict)));
    let pending:i64=sqlx::query_scalar("SELECT COUNT(*) FROM fm_batch_preflight WHERE batch=?").bind(batch.id.to_string()).fetch_one(&first.pool).await.unwrap();assert_eq!(pending,0);
    first.pool.close().await;second.pool.close().await;drop((first,second));temp.close().unwrap();assert!(!root.exists());
}

#[tokio::test]
async fn reservation_and_first_binding_roll_back_completely_when_their_transaction_fails() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().to_owned();let store=BatchStore::open(&root.join("batch.db")).await.unwrap();let batch=spec();
    sqlx::query("CREATE TRIGGER reject_reservation BEFORE INSERT ON fm_batch_preflight BEGIN SELECT RAISE(ABORT,'injected admission failure'); END").execute(&store.pool).await.unwrap();
    assert!(store.reserve_submission("owner",&batch).await.is_err());
    assert!(matches!(store.snapshot("owner",batch.id).await,Err(FileManagerError::NotFound)));
    sqlx::query("DROP TRIGGER reject_reservation").execute(&store.pool).await.unwrap();
    assert!(store.reserve_submission("owner",&batch).await.unwrap());
    let anchor=DirectoryReady {path:batch.destination.clone(),identity:"volume:original".into()};
    sqlx::query("CREATE TRIGGER reject_binding BEFORE DELETE ON fm_batch_preflight BEGIN SELECT RAISE(ABORT,'injected binding failure'); END").execute(&store.pool).await.unwrap();
    assert!(store.create_with_destination("owner",&batch,&anchor).await.is_err());
    let anchors:i64=sqlx::query_scalar("SELECT COUNT(*) FROM fm_batch_destinations WHERE batch=?").bind(batch.id.to_string()).fetch_one(&store.pool).await.unwrap();assert_eq!(anchors,0);
    let pending:i64=sqlx::query_scalar("SELECT COUNT(*) FROM fm_batch_preflight WHERE batch=?").bind(batch.id.to_string()).fetch_one(&store.pool).await.unwrap();assert_eq!(pending,1);
    sqlx::query("DROP TRIGGER reject_binding").execute(&store.pool).await.unwrap();
    assert!(!store.create_with_destination("owner",&batch,&anchor).await.unwrap());
    store.pool.close().await;drop(store);temp.close().unwrap();assert!(!root.exists());
}

#[tokio::test]
async fn a_finished_unbound_submission_cannot_adopt_a_directory_later() {
    let temp=tempfile::tempdir().unwrap();let root=temp.path().to_owned();let store=BatchStore::open(&root.join("batch.db")).await.unwrap();let batch=spec();
    store.reserve_submission("owner",&batch).await.unwrap();
    store.finish_scan("owner",batch.id,Some("preflight failed before any effects")).await.unwrap();
    let anchor=DirectoryReady {path:batch.destination.clone(),identity:"volume:later".into()};
    assert!(matches!(store.create_with_destination("owner",&batch,&anchor).await,Err(FileManagerError::Conflict)));
    assert_eq!(store.snapshot("owner",batch.id).await.unwrap().phase,"failed");
    assert!(store.items("owner",batch.id,0,128).await.unwrap().is_empty());
    store.pool.close().await;drop(store);temp.close().unwrap();assert!(!root.exists());
}

#[tokio::test]
async fn frozen_destination_is_atomic_owner_scoped_and_resolves_children_through_ready_parents() {
    let temp=tempfile::tempdir().unwrap();let db=temp.path().join("batch.db");let store=BatchStore::open(&db).await.unwrap();let mut batch=spec();batch.destination=NativeFilePath::Utf8("D:\\destination\\".into());
    let frozen=DirectoryReady {path:batch.destination.clone(),identity:"volume:root".into()};
    assert!(store.create_with_destination("owner",&batch,&frozen).await.unwrap());assert!(!store.create_with_destination("owner",&batch,&frozen).await.unwrap());
    let mut replacement=frozen.clone();replacement.identity="volume:replacement".into();assert!(store.create_with_destination("owner",&batch,&replacement).await.is_err());
    assert!(store.create_with_destination("foreign",&batch,&frozen).await.is_err());
    let parent=directory(0);let mut child=item(1);child.destination=NativeFilePath::Utf8("D:\\destination\\0\\1".into());
    store.append_planned_batch("owner",batch.id,&[PlannedItem {spec:parent.clone(),parent:None,skip:None},PlannedItem {spec:child.clone(),parent:Some(parent.id),skip:None}]).await.unwrap();store.finish_scan("owner",batch.id,None).await.unwrap();
    assert_eq!(store.item_destination_parent("owner",parent.id,&parent.destination,PathStyle::Windows).await.unwrap().identity,frozen.identity);
    assert!(store.item_destination_parent("owner",child.id,&child.destination,PathStyle::Windows).await.is_err(),"child must wait for actual parent readiness");
    let worker=RequestId::new();store.heartbeat("owner",worker).await.unwrap();let claimed=store.claim("owner",batch.id,worker).await.unwrap().unwrap();assert_eq!(claimed.spec.id,parent.id);
    let ready=DirectoryReady {path:NativeFilePath::Utf8("D:\\destination\\renamed".into()),identity:"volume:child".into()};
    store.save_item_target("owner",parent.id,worker,&ItemTarget {path:ready.path.clone(),overwrite:false,choice:FileConflictPolicy::KeepBoth}).await.unwrap();store.record_directory_ready("owner",parent.id,worker,&ready).await.unwrap();
    assert!(store.item_destination_parent("owner",child.id,&child.destination,PathStyle::Windows).await.is_err());
    let actual=store.item_target("owner",child.id,PathStyle::Windows).await.unwrap();let reopened=BatchStore::open(&db).await.unwrap();
    assert_eq!(reopened.item_destination_parent("owner",child.id,&actual.path,PathStyle::Windows).await.unwrap().identity,ready.identity);
    assert!(reopened.item_destination_parent("foreign",child.id,&actual.path,PathStyle::Windows).await.is_err());
    assert!(reopened.create_with_destination("owner",&batch,&replacement).await.is_err());
    let legacy=spec();store.create("owner",&legacy).await.unwrap();let mut legacy_frozen=frozen;legacy_frozen.path=legacy.destination.clone();assert!(store.create_with_destination("owner",&legacy,&legacy_frozen).await.is_err(),"old rows must not silently adopt current directories");
}
async fn finish_directory(store:&BatchStore,worker:RequestId,entry:&FileBatchItem) {
    let finalization=store.begin_directory_finalization("owner",entry.spec.id,worker).await.unwrap().unwrap();
    let FileManagerOperation::SetDirectoryMetadata {path,expected_object,metadata}=finalization.operation else {panic!()};
    let result=FileManagerResult::DirectoryMetadata {path,identity:expected_object,metadata:FileTransferMetadataResult {requested:metadata.clone(),actual:metadata,warnings:Vec::new()}};
    store.record_directory_outcome("owner",entry.spec.id,worker,finalization.request_id,&result).await.unwrap();
    let current=store.item("owner",entry.spec.id).await.unwrap();
    store.advance("owner",entry.spec.id,worker,current.revision,FileItemState::Completed,FileByteCount(0),None).await.unwrap();
}
async fn finish_file(store:&BatchStore,worker:RequestId,mut entry:FileBatchItem) {
    for state in [FileItemState::Verifying,FileItemState::Publishing,FileItemState::Completed] {
        entry=store.advance("owner",entry.spec.id,worker,entry.revision,state,entry.spec.size,None).await.unwrap();
    }
}
fn directory(n:u64)->FileBatchItemSpec {let mut spec=item(n);spec.kind=ManagedFileKind::Directory;spec.size=FileByteCount(0);spec.modified_at_unix_ms=Some(1234);spec.unix_mode=Some(0o500);spec}

#[tokio::test]
async fn directory_barriers_release_workers_and_finalize_only_after_descendants() {
    let temp=tempfile::tempdir().unwrap();let db=temp.path().join("batch.db");let store=BatchStore::open(&db).await.unwrap();let batch=spec();let worker=RequestId::new();
    let parent=directory(0);let nested=directory(1);let child=item(2);
    store.create("owner",&batch).await.unwrap();
    store.append_planned_batch("owner",batch.id,&[
        PlannedItem {spec:parent.clone(),parent:None,skip:None},PlannedItem {spec:nested.clone(),parent:Some(parent.id),skip:None},PlannedItem {spec:child.clone(),parent:Some(nested.id),skip:None},
    ]).await.unwrap();store.finish_scan("owner",batch.id,None).await.unwrap();store.heartbeat("owner",worker).await.unwrap();
    let top=store.claim("owner",batch.id,worker).await.unwrap().unwrap();ready_directory(&store,worker,&top).await;
    assert_eq!(store.item("owner",parent.id).await.unwrap().state,FileItemState::WaitingChildren);
    assert_eq!(store.snapshot("owner",batch.id).await.unwrap().completed.0,0);
    let inner=store.claim("owner",batch.id,worker).await.unwrap().unwrap();assert_eq!(inner.spec.id,nested.id);ready_directory(&store,worker,&inner).await;
    let file=store.claim("owner",batch.id,worker).await.unwrap().unwrap();assert_eq!(file.spec.id,child.id);
    assert!(store.claim("owner",batch.id,worker).await.unwrap().is_none());finish_file(&store,worker,file).await;
    let inner=store.claim("owner",batch.id,worker).await.unwrap().unwrap();assert_eq!(inner.spec.id,nested.id);finish_directory(&store,worker,&inner).await;
    let top=store.claim("owner",batch.id,worker).await.unwrap().unwrap();assert_eq!(top.spec.id,parent.id);finish_directory(&store,worker,&top).await;
    assert_eq!(store.snapshot("owner",batch.id).await.unwrap().phase,"completed");
    let reopened=BatchStore::open(&db).await.unwrap();assert!(reopened.item("owner",parent.id).await.unwrap().metadata_result.is_some());
    assert_eq!(reopened.items_filtered("owner",batch.id,0,Some(FileItemState::Completed)).await.unwrap().iter().filter(|item|item.metadata_result.is_some()).count(),2);
    assert!(reopened.directory_ready("other",parent.id).await.is_err());
}

#[tokio::test]
async fn directory_pause_cancel_and_failed_child_retry_preserve_preparation_and_completed_files() {
    let temp=tempfile::tempdir().unwrap();let store=BatchStore::open(&temp.path().join("batch.db")).await.unwrap();let batch=spec();let worker=RequestId::new();
    let parent=directory(0);let good=item(1);let bad=item(2);store.create("owner",&batch).await.unwrap();
    store.append_planned_batch("owner",batch.id,&[PlannedItem {spec:parent.clone(),parent:None,skip:None},PlannedItem {spec:good.clone(),parent:Some(parent.id),skip:None},PlannedItem {spec:bad.clone(),parent:Some(parent.id),skip:None}]).await.unwrap();store.finish_scan("owner",batch.id,None).await.unwrap();store.heartbeat("owner",worker).await.unwrap();
    let top=store.claim("owner",batch.id,worker).await.unwrap().unwrap();ready_directory(&store,worker,&top).await;
    store.set_intent("owner",batch.id,FileTransferIntent::Pause).await.unwrap();assert_eq!(store.snapshot("owner",batch.id).await.unwrap().phase,"paused");assert!(store.claim("owner",batch.id,worker).await.unwrap().is_none());
    store.set_intent("owner",batch.id,FileTransferIntent::Run).await.unwrap();assert_eq!(store.item("owner",parent.id).await.unwrap().state,FileItemState::WaitingChildren);
    let file=store.claim("owner",batch.id,worker).await.unwrap().unwrap();let good_attempt=file.attempt;finish_file(&store,worker,file).await;
    let file=store.claim("owner",batch.id,worker).await.unwrap().unwrap();store.advance("owner",file.spec.id,worker,file.revision,FileItemState::Failed,FileByteCount(0),Some("fixture failure")).await.unwrap();store.propagate_blocked_children("owner",batch.id).await.unwrap();
    assert_eq!(store.item("owner",parent.id).await.unwrap().state,FileItemState::Failed);
    assert!(store.directory_finalization("owner",top.attempt.unwrap()).await.unwrap().is_none());
    assert_eq!(store.retry_failed("owner",batch.id).await.unwrap(),2);
    let file=store.claim("owner",batch.id,worker).await.unwrap().unwrap();assert_eq!(file.spec.id,bad.id);finish_file(&store,worker,file).await;
    assert_eq!(store.item("owner",good.id).await.unwrap().attempt,good_attempt);
    let top=store.claim("owner",batch.id,worker).await.unwrap().unwrap();
    store.set_intent("owner",batch.id,FileTransferIntent::Pause).await.unwrap();assert!(store.begin_directory_finalization("owner",top.spec.id,worker).await.unwrap().is_none());
    let top=store.item("owner",top.spec.id).await.unwrap();store.advance("owner",top.spec.id,worker,top.revision,FileItemState::Paused,FileByteCount(0),None).await.unwrap();
    store.set_intent("owner",batch.id,FileTransferIntent::Cancel).await.unwrap();assert_eq!(store.snapshot("owner",batch.id).await.unwrap().phase,"cancelled");
    assert!(store.directory_ready("owner",parent.id).await.unwrap().is_some());
}

#[tokio::test]
async fn directory_finalization_recovery_keeps_its_original_request_and_validated_receipt() {
    let temp=tempfile::tempdir().unwrap();let db=temp.path().join("batch.db");let store=BatchStore::open(&db).await.unwrap();let batch=spec();let worker=RequestId::new();let parent=directory(0);
    store.create("owner",&batch).await.unwrap();store.append("owner",batch.id,&[parent.clone()]).await.unwrap();store.finish_scan("owner",batch.id,None).await.unwrap();store.heartbeat("owner",worker).await.unwrap();
    let top=store.claim("owner",batch.id,worker).await.unwrap().unwrap();ready_directory(&store,worker,&top).await;
    let top=store.claim("owner",batch.id,worker).await.unwrap().unwrap();let finalization=store.begin_directory_finalization("owner",parent.id,worker).await.unwrap().unwrap();
    assert_eq!(finalization.request_id,top.attempt.unwrap());
    store.set_intent("owner",batch.id,FileTransferIntent::Cancel).await.unwrap();assert_eq!(store.item("owner",parent.id).await.unwrap().state,FileItemState::Publishing);
    store.forget_worker_for_test(worker).await;let reopened=BatchStore::open(&db).await.unwrap();assert_eq!(reopened.recover_stale("owner").await.unwrap(),1);
    let observer=RequestId::new();reopened.heartbeat("owner",observer).await.unwrap();let item=reopened.claim_for_observation("owner",parent.id,observer).await.unwrap();assert_eq!(item.attempt,Some(finalization.request_id));
    assert_eq!(reopened.directory_finalization("owner",finalization.request_id).await.unwrap().unwrap().operation,finalization.operation);
    let FileManagerOperation::SetDirectoryMetadata {path,expected_object,metadata}=finalization.operation else {panic!()};
    let mut result=FileManagerResult::DirectoryMetadata {path,identity:format!("{expected_object}wrong"),metadata:FileTransferMetadataResult {requested:metadata.clone(),actual:metadata,warnings:Vec::new()}};
    assert!(reopened.record_directory_outcome("owner",parent.id,observer,finalization.request_id,&result).await.is_err());
    let FileManagerResult::DirectoryMetadata {identity,..}=&mut result else {panic!()};*identity=expected_object;
    reopened.record_directory_outcome("owner",parent.id,observer,finalization.request_id,&result).await.unwrap();
    let item=reopened.item("owner",parent.id).await.unwrap();reopened.advance("owner",parent.id,observer,item.revision,FileItemState::Completed,FileByteCount(0),None).await.unwrap();
    assert_eq!(reopened.snapshot("owner",batch.id).await.unwrap().phase,"completed");
}

#[tokio::test]
async fn delayed_progress_never_regresses_publication_evidence_or_cancels_the_original_attempt() {
    let temp=tempfile::tempdir().unwrap();let store=BatchStore::open(&temp.path().join("evidence.db")).await.unwrap();
    let batch=spec();let worker=RequestId::new();let mut source=item(0);source.modified_at_unix_ms=Some(123);store.create("owner",&batch).await.unwrap();store.append("owner",batch.id,&[source]).await.unwrap();store.finish_scan("owner",batch.id,None).await.unwrap();store.heartbeat("owner",worker).await.unwrap();
    let entry=store.claim("owner",batch.id,worker).await.unwrap().unwrap();let attempt=entry.attempt.unwrap();
    let context=ExecutionContext {os_family:OsFamily::Windows,os_name:"fixture".into(),os_version:"1".into(),architecture:CpuArchitecture::X86_64,execution_scope:ExecutionScope::Native,identity:None,path_style:PathStyle::Windows,interpreter:None,cwd:None,environment_revision:"fixture-v1".into()};
    let mut receipt=TransferSnapshot {metadata_result:None,request_id:attempt,initiated_by:OperatorRef::account(UserId::from_u128(3),EndpointKey::new([3;32])),initiating_user:None,execution_context:Some(context.clone()),path:"D:\\destination\\0".into(),direction:"receive".into(),state:"running".into(),offset:32,size:100,sha256:Some("a".repeat(64)),finished_at_unix_ms:None,message:None,published:None};
    let old=AttemptEvidence {digest:receipt.sha256.clone(),context:Some(context),receipt:Some(receipt.clone()),local_checkpoint:Some(FileByteCount(32)),..Default::default()};
    store.observe_attempt("owner",entry.spec.id,worker,attempt,&old).await.unwrap();
    receipt.offset=80;let mut newest=AttemptEvidence {receipt:Some(receipt.clone()),local_checkpoint:Some(FileByteCount(80)),..old.clone()};
    store.observe_attempt("owner",entry.spec.id,worker,attempt,&newest).await.unwrap();store.observe_attempt("owner",entry.spec.id,worker,attempt,&old).await.unwrap();
    assert_eq!(store.attempt_evidence("owner",attempt).await.unwrap().receipt.unwrap().offset,80);
    receipt.state="committing".into();receipt.offset=100;receipt.published=Some(true);
    newest.receipt=Some(receipt.clone());
    store.observe_attempt("owner",entry.spec.id,worker,attempt,&newest).await.unwrap();
    let published=newest.clone();
    let metadata=FileTransferMetadata {modified_at_unix_ms:Some(123),unix_mode:None};
    receipt.metadata_result=Some(FileTransferMetadataResult {requested:metadata.clone(),actual:metadata,warnings:vec![]});
    newest.receipt=Some(receipt.clone());
    store.observe_attempt("owner",entry.spec.id,worker,attempt,&newest).await.unwrap();
    store.observe_attempt("owner",entry.spec.id,worker,attempt,&published).await.unwrap();
    assert_eq!(store.attempt_evidence("owner",attempt).await.unwrap().receipt,Some(receipt.clone()));
    let mut foreign=newest.clone();
    let result=foreign.receipt.as_mut().unwrap().metadata_result.as_mut().unwrap();
    result.requested.modified_at_unix_ms=Some(124);result.actual.modified_at_unix_ms=Some(124);
    assert!(store.observe_attempt("owner",entry.spec.id,worker,attempt,&foreign).await.is_err());
    receipt.state="completed".into();receipt.finished_at_unix_ms=Some(123);
    newest.receipt=Some(receipt);newest.local_checkpoint=Some(FileByteCount(100));
    store.observe_attempt("owner",entry.spec.id,worker,attempt,&newest).await.unwrap();
    let before=serde_json::to_value(store.attempt_evidence("owner",attempt).await.unwrap()).unwrap();
    store.observe_attempt("owner",entry.spec.id,worker,attempt,&old).await.unwrap();assert_eq!(serde_json::to_value(store.attempt_evidence("owner",attempt).await.unwrap()).unwrap(),before);
    let mut changed=old.clone();changed.receipt.as_mut().unwrap().path="D:\\other".into();assert!(store.observe_attempt("owner",entry.spec.id,worker,attempt,&changed).await.is_err());
    changed=old.clone();changed.digest=Some("b".repeat(64));assert!(store.observe_attempt("owner",entry.spec.id,worker,attempt,&changed).await.is_err());
    changed=newest.clone();changed.receipt.as_mut().unwrap().state="failed".into();assert!(store.observe_attempt("owner",entry.spec.id,worker,attempt,&changed).await.is_err());
    changed=old;changed.local_checkpoint=Some(FileByteCount(101));assert!(store.observe_attempt("owner",entry.spec.id,worker,attempt,&changed).await.is_err());
    assert_eq!(serde_json::to_value(store.attempt_evidence("owner",attempt).await.unwrap()).unwrap(),before);
}

#[tokio::test]
async fn wholly_failed_batch_is_distinct_from_partial_success() {
    let dir=tempfile::tempdir().unwrap();let store=BatchStore::open(&dir.path().join("failures.db")).await.unwrap();let worker=RequestId::new();
    store.heartbeat("owner",worker).await.unwrap();
    for count in [1,2] {
        let batch=spec();store.create("owner",&batch).await.unwrap();let items:Vec<_>=(0..count).map(item).collect();store.append("owner",batch.id,&items).await.unwrap();store.finish_scan("owner",batch.id,None).await.unwrap();
        for _ in 0..count {let current=store.claim("owner",batch.id,worker).await.unwrap().unwrap();store.advance("owner",current.spec.id,worker,current.revision,FileItemState::Failed,FileByteCount(0),Some("access denied")).await.unwrap();}
        let snapshot=store.snapshot("owner",batch.id).await.unwrap();assert_eq!(snapshot.phase,"failed");assert_eq!(snapshot.failed.0,count);
    }
}

#[tokio::test]
async fn local_publication_and_attribute_outcomes_are_bound_and_survive_reopening() {
    let temp=tempfile::tempdir().unwrap();let path=temp.path().join("local-facts.db");let store=BatchStore::open(&path).await.unwrap();let mut batch=spec();batch.direction=FileTransferDirection::Download;
    let worker=RequestId::new();let mut source=item(0);source.modified_at_unix_ms=Some(123);source.unix_mode=Some(0o640);
    store.create("owner",&batch).await.unwrap();store.append("owner",batch.id,&[source.clone()]).await.unwrap();store.finish_scan("owner",batch.id,None).await.unwrap();store.heartbeat("owner",worker).await.unwrap();let entry=store.claim("owner",batch.id,worker).await.unwrap().unwrap();let attempt=entry.attempt.unwrap();
    let context=ExecutionContext {os_family:OsFamily::Windows,os_name:"fixture".into(),os_version:"1".into(),architecture:CpuArchitecture::X86_64,execution_scope:ExecutionScope::Native,identity:None,path_style:PathStyle::Windows,interpreter:None,cwd:None,environment_revision:"fixture-v1".into()};
    let metadata=FileTransferMetadata {modified_at_unix_ms:source.modified_at_unix_ms,unix_mode:source.unix_mode};
    let intent=AttemptEvidence {digest:Some("a".repeat(64)),context:Some(context),local_checkpoint:Some(source.size),local_publication_intent:true,local_metadata_requested:Some(metadata.clone()),..Default::default()};
    let publication=AttemptEvidence {local_published:true,..Default::default()};
    assert!(store.observe_attempt("owner",source.id,worker,attempt,&publication).await.is_err());
    store.observe_attempt("owner",source.id,worker,attempt,&intent).await.unwrap();
    let result=FileTransferMetadataResult {requested:metadata.clone(),actual:metadata,warnings:vec![]};
    let attributes=AttemptEvidence {local_metadata_result:Some(result.clone()),..Default::default()};
    assert!(store.observe_attempt("owner",source.id,worker,attempt,&attributes).await.is_err());
    store.observe_attempt("owner",source.id,worker,attempt,&publication).await.unwrap();
    let pending=store.attempt_evidence("owner",attempt).await.unwrap();assert!(pending.local_published);assert!(pending.local_metadata_result.is_none());
    store.observe_attempt("owner",source.id,worker,attempt,&attributes).await.unwrap();
    store.observe_attempt("owner",source.id,worker,attempt,&intent).await.unwrap();
    let mut altered=attributes.clone();altered.local_metadata_result.as_mut().unwrap().actual.unix_mode=Some(0o600);assert!(store.observe_attempt("owner",source.id,worker,attempt,&altered).await.is_err());
    altered=attributes.clone();let wrong=altered.local_metadata_result.as_mut().unwrap();wrong.requested.modified_at_unix_ms=Some(124);wrong.actual.modified_at_unix_ms=Some(124);assert!(store.observe_attempt("owner",source.id,worker,attempt,&altered).await.is_err());
    let reopened=BatchStore::open(&path).await.unwrap();let settled=reopened.attempt_evidence("owner",attempt).await.unwrap();assert!(settled.local_publication_intent && settled.local_published);assert_eq!(settled.local_metadata_result,Some(result.clone()));
    assert_eq!(reopened.item("owner",source.id).await.unwrap().metadata_result,Some(result.clone()));
    assert_eq!(reopened.items("owner",batch.id,0,128).await.unwrap()[0].metadata_result,Some(result.clone()));
    assert_eq!(reopened.items_filtered("owner",batch.id,0,None).await.unwrap()[0].metadata_result,Some(result));
    assert!(reopened.items_filtered("other-owner",batch.id,0,None).await.unwrap().is_empty());
    assert!(reopened.attempt_evidence("other-owner",attempt).await.is_err());
}

#[tokio::test]
async fn bounded_history_keeps_older_unfinished_batches_visible() {
    let dir=tempfile::tempdir().unwrap();let store=BatchStore::open(&dir.path().join("history.db")).await.unwrap();
    let active=spec();store.create("owner",&active).await.unwrap();
    sqlx::query("UPDATE fm_batches SET created=1 WHERE id=?").bind(active.id.to_string()).execute(&store.pool).await.unwrap();
    for _ in 0..3 {let done=spec();store.create("owner",&done).await.unwrap();store.finish_scan("owner",done.id,None).await.unwrap();}
    assert_eq!(store.list("owner",1).await.unwrap()[0].spec.id,active.id);
    store.set_intent("owner",active.id,FileTransferIntent::Cancel).await.unwrap();store.acknowledge_scan_stopped("owner",active.id).await.unwrap();
    let newest=spec();store.create("owner",&newest).await.unwrap();store.finish_scan("owner",newest.id,None).await.unwrap();
    sqlx::query("UPDATE fm_batches SET created=created+60000 WHERE id=?").bind(newest.id.to_string()).execute(&store.pool).await.unwrap();
    assert_eq!(store.list("owner",1).await.unwrap()[0].spec.id,newest.id);
}

#[tokio::test]
async fn explicit_retry_preserves_completed_skipped_and_uncertain_attempts() {
    let dir=tempfile::tempdir().unwrap();let store=BatchStore::open(&dir.path().join("retry.db")).await.unwrap();
    let batch=spec();let items:Vec<_>=(0..3).map(item).collect();let worker=RequestId::new();
    store.create("owner",&batch).await.unwrap();store.append("owner",batch.id,&items).await.unwrap();store.finish_scan("owner",batch.id,None).await.unwrap();store.heartbeat("owner",worker).await.unwrap();
    let first=store.claim("owner",batch.id,worker).await.unwrap().unwrap();
    let failed=store.advance("owner",first.spec.id,worker,first.revision,FileItemState::Failed,FileByteCount(0),Some("access denied")).await.unwrap();
    let second=store.claim("owner",batch.id,worker).await.unwrap().unwrap();
    assert!(store.retry_failed("owner",batch.id).await.is_err(),"an active attempt cannot be replayed by retry");
    store.advance("owner",second.spec.id,worker,second.revision,FileItemState::Skipped,FileByteCount(0),None).await.unwrap();
    let third=store.claim("owner",batch.id,worker).await.unwrap().unwrap();
    let verifying=store.advance("owner",third.spec.id,worker,third.revision,FileItemState::Verifying,FileByteCount(100),None).await.unwrap();
    let publishing=store.advance("owner",third.spec.id,worker,verifying.revision,FileItemState::Publishing,FileByteCount(100),None).await.unwrap();
    let completed=store.advance("owner",third.spec.id,worker,publishing.revision,FileItemState::Completed,FileByteCount(100),None).await.unwrap();
    assert!(store.retry_failed("other",batch.id).await.is_err());assert_eq!(store.retry_failed("owner",batch.id).await.unwrap(),1);
    assert_eq!(store.item("owner",completed.spec.id).await.unwrap(),completed);
    assert_eq!(store.item("owner",second.spec.id).await.unwrap().state,FileItemState::Skipped);
    let retried=store.claim("owner",batch.id,worker).await.unwrap().unwrap();assert_ne!(retried.attempt,failed.attempt);assert_eq!(retried.previous_attempt,failed.attempt);
    let unknown=store.advance("owner",retried.spec.id,worker,retried.revision,FileItemState::Reconciling,FileByteCount(0),Some("lost reply")).await.unwrap();
    store.release_for_observation("owner",unknown.spec.id,worker).await.unwrap();assert!(store.retry_failed("owner",batch.id).await.is_err());
    assert_eq!(store.item("owner",unknown.spec.id).await.unwrap().attempt,unknown.attempt);
}

#[tokio::test]
async fn runner_lease_prevents_other_live_instances_from_controlling_the_batch() {
    let dir=tempfile::tempdir().unwrap();let db=dir.path().join("lease.db");let store=BatchStore::open(&db).await.unwrap();let other=BatchStore::open(&db).await.unwrap();
    let batch=spec();let first=RequestId::new();let second=RequestId::new();store.create("owner",&batch).await.unwrap();store.heartbeat("owner",first).await.unwrap();other.heartbeat("owner",second).await.unwrap();
    store.claim_runner("owner",batch.id,first).await.unwrap();assert!(other.claim_runner("owner",batch.id,second).await.is_err());assert!(other.verify_runner_control("owner",batch.id,second).await.is_err());
    assert!(other.verify_runner_control("another-account",batch.id,first).await.is_err());
    sqlx::query("UPDATE fm_workers SET heartbeat=0 WHERE id=?").bind(first.to_string()).execute(&store.pool).await.unwrap();
    other.claim_runner("owner",batch.id,second).await.unwrap();assert!(store.verify_runner_control("owner",batch.id,first).await.is_err());
    other.release_runner("owner",batch.id,second).await.unwrap();store.verify_runner_control("owner",batch.id,first).await.unwrap();
}

#[tokio::test]
async fn coordinator_failure_releases_only_its_own_attempt_and_preserves_evidence() {
    let dir=tempfile::tempdir().unwrap();let store=BatchStore::open(&dir.path().join("retire.db")).await.unwrap();
    let first=spec();let second=spec();let worker=RequestId::new();let other=RequestId::new();
    for batch in [&first,&second] {
        store.create("owner",batch).await.unwrap();store.append("owner",batch.id,&[item(0)]).await.unwrap();store.finish_scan("owner",batch.id,None).await.unwrap();
    }
    store.heartbeat("owner",worker).await.unwrap();store.heartbeat("owner",other).await.unwrap();
    store.claim_runner("owner",first.id,worker).await.unwrap();store.claim_runner("owner",second.id,other).await.unwrap();
    let claimed=store.claim("owner",first.id,worker).await.unwrap().unwrap();let unrelated=store.claim("owner",second.id,other).await.unwrap().unwrap();
    let active=store.advance("owner",claimed.spec.id,worker,claimed.revision,FileItemState::Transferring,FileByteCount(80),None).await.unwrap();
    assert!(store.retire_worker_with_reason("wrong-owner",first.id,worker,Some("failure")).await.is_err());
    store.retire_worker_with_reason("owner",first.id,worker,Some("coordinator connection lost")).await.unwrap();
    let retired=store.item("owner",claimed.spec.id).await.unwrap();assert_eq!(retired.state,FileItemState::Reconciling);assert_eq!(retired.attempt,active.attempt);assert_eq!(retired.confirmed,FileByteCount(80));assert_eq!(retired.message.as_deref(),Some("coordinator connection lost"));
    assert_eq!(store.snapshot("owner",first.id).await.unwrap().phase,"reconciling");assert_eq!(store.item("owner",unrelated.spec.id).await.unwrap(),unrelated);
    assert!(store.release_runner("owner",first.id,worker).await.is_ok());
    store.heartbeat("owner",worker).await.unwrap();store.claim_runner("owner",first.id,worker).await.unwrap();
    let recovered=store.claim_for_observation("owner",retired.spec.id,worker).await.unwrap();assert_eq!(recovered.attempt,active.attempt,"observation must retain the original attempt");
}

#[tokio::test]
async fn durable_submission_deduplicates_and_preserves_owner_and_spec() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tasks.db");
    let store = BatchStore::open(&db).await.unwrap();
    let batch = spec();
    let (a, b) = tokio::join!(store.create("a", &batch), store.create("a", &batch));
    assert_ne!(a.unwrap(), b.unwrap());
    assert!(matches!(
        store.create("b", &batch).await,
        Err(FileManagerError::NotFound)
    ));
    let mut changed = batch.clone();
    changed.force_relay = false;
    assert!(matches!(
        store.create("a", &changed).await,
        Err(FileManagerError::Conflict)
    ));
    let entry = item(0);
    store.append("a", batch.id, &[entry.clone()]).await.unwrap();
    store.finish_scan("a", batch.id, None).await.unwrap();
    drop(store);
    let store = BatchStore::open(&db).await.unwrap();
    assert_eq!(store.snapshot("a", batch.id).await.unwrap().spec, batch);
    assert_eq!(store.item("a", entry.id).await.unwrap().spec, entry);
    assert!(store.item("b", entry.id).await.is_err());
}

#[tokio::test]
async fn pause_requires_worker_confirmation_and_resume_has_a_new_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let store = BatchStore::open(&dir.path().join("tasks.db"))
        .await
        .unwrap();
    let batch = spec();
    let entry = item(0);
    let worker = RequestId::new();
    store.create("a", &batch).await.unwrap();
    store
        .set_intent("a", batch.id, FileTransferIntent::Pause)
        .await
        .unwrap();
    assert_eq!(
        store.snapshot("a", batch.id).await.unwrap().phase,
        "pause_requested"
    );
    store.acknowledge_scan_stopped("a", batch.id).await.unwrap();
    assert_eq!(store.snapshot("a", batch.id).await.unwrap().phase, "paused");
    store
        .set_intent("a", batch.id, FileTransferIntent::Run)
        .await
        .unwrap();
    store.append("a", batch.id, &[entry.clone()]).await.unwrap();
    store.finish_scan("a", batch.id, None).await.unwrap();
    store.heartbeat("a", worker).await.unwrap();
    let first = store.claim("a", batch.id, worker).await.unwrap().unwrap();
    let active = store
        .advance(
            "a",
            entry.id,
            worker,
            first.revision,
            FileItemState::Transferring,
            FileByteCount(25),
            None,
        )
        .await
        .unwrap();
    store
        .set_intent("a", batch.id, FileTransferIntent::Pause)
        .await
        .unwrap();
    assert_eq!(
        store.snapshot("a", batch.id).await.unwrap().phase,
        "pause_requested"
    );
    assert!(
        store
            .advance(
                "a",
                entry.id,
                worker,
                active.revision,
                FileItemState::Paused,
                FileByteCount(25),
                None
            )
            .await
            .is_err()
    );
    let stopping = store.item("a", entry.id).await.unwrap();
    store
        .advance(
            "a",
            entry.id,
            worker,
            stopping.revision,
            FileItemState::Paused,
            FileByteCount(25),
            None,
        )
        .await
        .unwrap();
    assert_eq!(store.snapshot("a", batch.id).await.unwrap().phase, "paused");
    store
        .set_intent("a", batch.id, FileTransferIntent::Run)
        .await
        .unwrap();
    let resumed = store.claim("a", batch.id, worker).await.unwrap().unwrap();
    assert_ne!(resumed.attempt, first.attempt);
    assert_eq!(resumed.previous_attempt, first.attempt);
    assert_eq!(resumed.confirmed.0, 25);
}

#[tokio::test]
async fn publication_wins_late_cancel_and_terminal_progress_never_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let store = BatchStore::open(&dir.path().join("tasks.db"))
        .await
        .unwrap();
    let batch = spec();
    let entry = item(0);
    let worker = RequestId::new();
    store.create("a", &batch).await.unwrap();
    store.append("a", batch.id, &[entry.clone()]).await.unwrap();
    store.finish_scan("a", batch.id, None).await.unwrap();
    store.heartbeat("a", worker).await.unwrap();
    let mut current = store.claim("a", batch.id, worker).await.unwrap().unwrap();
    for state in [
        FileItemState::Transferring,
        FileItemState::Verifying,
        FileItemState::Publishing,
    ] {
        current = store
            .advance(
                "a",
                entry.id,
                worker,
                current.revision,
                state,
                FileByteCount(100),
                None,
            )
            .await
            .unwrap();
    }
    store
        .set_intent("a", batch.id, FileTransferIntent::Cancel)
        .await
        .unwrap();
    assert_eq!(
        store.item("a", entry.id).await.unwrap().state,
        FileItemState::Publishing
    );
    let done = store
        .advance(
            "a",
            entry.id,
            worker,
            current.revision,
            FileItemState::Completed,
            FileByteCount(100),
            None,
        )
        .await
        .unwrap();
    assert!(
        store
            .advance(
                "a",
                entry.id,
                worker,
                done.revision,
                FileItemState::Transferring,
                FileByteCount(99),
                None
            )
            .await
            .is_err()
    );
    assert_eq!(
        store.snapshot("a", batch.id).await.unwrap().phase,
        "completed"
    );
}

#[tokio::test]
async fn claims_obey_device_limit_and_crashed_work_only_reconciles() {
    let dir = tempfile::tempdir().unwrap();
    let store = BatchStore::open(&dir.path().join("tasks.db"))
        .await
        .unwrap();
    let batch = spec();
    let worker = RequestId::new();
    store.create("a", &batch).await.unwrap();
    let items: Vec<_> = (0..3).map(item).collect();
    store.append("a", batch.id, &items).await.unwrap();
    store.finish_scan("a", batch.id, None).await.unwrap();
    store.heartbeat("a", worker).await.unwrap();
    assert!(store.claim("a", batch.id, worker).await.unwrap().is_some());
    assert!(store.claim("a", batch.id, worker).await.unwrap().is_some());
    assert!(store.claim("a", batch.id, worker).await.unwrap().is_none());
    assert_eq!(store.recover_stale("a").await.unwrap(), 0);
    // A separate dead owner has no live heartbeat. Removing its lease simulates
    // process loss deterministically, without sleeping or killing real services.
    store.forget_worker_for_test(worker).await;
    assert_eq!(store.recover_stale("a").await.unwrap(), 2);
    assert_eq!(
        store.item("a", items[0].id).await.unwrap().state,
        FileItemState::Reconciling
    );
    assert_eq!(
        store.snapshot("a", batch.id).await.unwrap().phase,
        "reconciling"
    );
}
#[tokio::test]
async fn device_history_scope_precedes_limit_and_clear_preserves_other_devices() {
    let temp=tempfile::tempdir().unwrap();let store=BatchStore::open(&temp.path().join("scoped.db")).await.unwrap();
    let first=spec();let mut second=spec();second.device.device_id=DeviceId::from_u128(3);
    let mut foreign=spec();foreign.device.tenant_id=TenantId::from_u128(4);
    for batch in [&first,&second,&foreign] {store.create("owner",batch).await.unwrap();store.finish_scan("owner",batch.id,None).await.unwrap();}
    let rows=store.list_visible_for_device("owner",first.device,1).await.unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].spec.id,first.id);
    assert!(store.list_visible_for_device("other",first.device,100).await.unwrap().is_empty());
    assert_eq!(store.clear_completed_for_device("owner",first.device).await.unwrap(),vec![first.id.to_string()]);
    assert!(store.list_visible_for_device("owner",first.device,100).await.unwrap().is_empty());
    assert_eq!(store.list_visible("owner",100).await.unwrap().len(),2);
    assert_eq!(store.list("owner",100).await.unwrap().len(),3);
}
