use super::*;
use pab_protocol::{DeploymentId, DeviceId, EndpointKey, OperatorRef, TenantId, UserId};

fn request() -> TransferRequest {
    TransferRequest {
        request_id: RequestId::new(),
        device_ref: DeviceRef {
            deployment_id: DeploymentId::from_u128(1),
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        },
        device_code: "123456789".parse().unwrap(),
        direction: "upload".into(),
        source: "local.bin".into(),
        destination: "/tmp/remote.bin".into(),
        overwrite: true,
    }
}

async fn queue(path: &Path, session: &str) -> TransferQueue {
    TransferQueue::open(path, session.into(), "guest".into())
        .await
        .unwrap()
}

fn remote(spec: &TransferRequest, state: &str, published: Option<bool>) -> TransferSnapshot {
    TransferSnapshot {
        request_id: spec.request_id,
        initiated_by: OperatorRef::account(UserId::from_u128(7), EndpointKey::new([7; 32])),
        direction: "receive".into(),
        path: spec.destination.clone(),
        state: state.into(),
        offset: 42,
        size: 42,
        sha256: Some("a".repeat(64)),
        finished_at_unix_ms: Some(100),
        message: None,
        published,
    }
}

#[tokio::test]
async fn deduplicates_atomically_and_survives_terminal_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let first = queue(&path, "first").await;
    let spec = request();
    let (a, b) = tokio::join!(first.submit(&spec), first.submit(&spec));
    assert_ne!(a.unwrap().1, b.unwrap().1);
    let mut changed = spec.clone();
    changed.overwrite = false;
    assert!(matches!(
        first.submit(&changed).await,
        Err(RuntimeStoreError::RequestConflict)
    ));
    first
        .finish(spec.request_id, "completed", None)
        .await
        .unwrap();
    first
        .finish(spec.request_id, "cancelled", None)
        .await
        .unwrap();
    first.close().await.unwrap();
    let second = queue(&path, "second").await;
    let (record, inserted) = second.submit(&spec).await.unwrap();
    assert!(!inserted);
    assert!(!record.owned);
    assert_eq!(record.operation.state, "completed");
    assert_eq!(record.phase, "completed");
    assert!(
        second
            .cancel(spec.request_id, spec.device_code)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn live_sessions_and_device_codes_are_isolated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let first = queue(&path, "first").await;
    let second = queue(&path, "second").await;
    let spec = request();
    first.submit(&spec).await.unwrap();
    assert!(second.get(spec.request_id, spec.device_code).await.is_err());
    assert!(
        second
            .cancel(spec.request_id, spec.device_code)
            .await
            .is_err()
    );
    assert!(second.page(None, None, None, 20).await.unwrap().is_empty());
    assert!(
        first
            .get(spec.request_id, "987654321".parse().unwrap())
            .await
            .is_err()
    );
    let other_actor = TransferQueue::open(&path, "third".into(), "account:another".into())
        .await
        .unwrap();
    first.close().await.unwrap();
    assert!(
        other_actor
            .get(spec.request_id, spec.device_code)
            .await
            .is_err()
    );
    assert!(second.get(spec.request_id, spec.device_code).await.is_ok());
}

#[tokio::test]
async fn cancellation_intent_cannot_reverse_confirmed_publication() {
    let dir = tempfile::tempdir().unwrap();
    let q = queue(&dir.path().join("db"), "first").await;
    let spec = request();
    q.submit(&spec).await.unwrap();
    q.phase(spec.request_id, "unconfirmed", Some(&"a".repeat(64)))
        .await
        .unwrap();
    let record = q.cancel(spec.request_id, spec.device_code).await.unwrap();
    assert_eq!(record.operation.state, "cancel_requested");
    assert!(record.operation.finished_at_unix_ms.is_none());
    q.reconcile(&record, &remote(&spec, "completed", Some(true)))
        .await
        .unwrap();
    q.finish(spec.request_id, "cancelled", None).await.unwrap();
    let record = q.get(spec.request_id, spec.device_code).await.unwrap();
    assert_eq!(record.operation.state, "completed");
    assert_eq!(record.phase, "completed");
    assert_eq!(record.operation.offset, 42);
    assert_eq!(record.operation.execution_observation, None);
}

#[tokio::test]
async fn a_full_transfer_with_unknown_publication_stays_unconfirmed() {
    let dir = tempfile::tempdir().unwrap();
    let q = queue(&dir.path().join("db"), "first").await;
    let spec = request();
    q.submit(&spec).await.unwrap();
    q.phase(spec.request_id, "unconfirmed", Some(&"a".repeat(64)))
        .await
        .unwrap();
    q.note(spec.request_id, "receipt lost after transfer")
        .await
        .unwrap();
    let record = q.get(spec.request_id, spec.device_code).await.unwrap();
    assert_eq!(
        record.operation.message.as_deref(),
        Some("receipt lost after transfer")
    );
    assert_eq!(
        record.operation.execution_observation.as_deref(),
        Some("unconfirmed")
    );
    q.reconcile(&record, &remote(&spec, "failed", None))
        .await
        .unwrap();
    assert!(
        q.get(spec.request_id, spec.device_code)
            .await
            .unwrap()
            .operation
            .finished_at_unix_ms
            .is_none()
    );
    let mut mismatch = remote(&spec, "completed", Some(true));
    mismatch.sha256 = Some("b".repeat(64));
    assert!(q.reconcile(&record, &mismatch).await.is_err());
    mismatch = remote(&spec, "completed", Some(true));
    mismatch.path = "/other".into();
    assert!(q.reconcile(&record, &mismatch).await.is_err());
    let record = q.cancel(spec.request_id, spec.device_code).await.unwrap();
    q.reconcile(&record, &remote(&spec, "failed", Some(false)))
        .await
        .unwrap();
    assert_eq!(
        q.get(spec.request_id, spec.device_code)
            .await
            .unwrap()
            .operation
            .state,
        "cancelled"
    );
}

#[tokio::test]
async fn downloads_claim_paths_across_processes_until_the_outcome_is_confirmed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let first = queue(&path, "first").await;
    let second = queue(&path, "second").await;
    let mut spec = request();
    spec.direction = "download".into();
    spec.destination = dir.path().join("result.bin").to_string_lossy().into_owned();
    first.submit(&spec).await.unwrap();
    let mut other = spec.clone();
    other.request_id = RequestId::new();
    assert!(second.submit(&other).await.is_err());
    first
        .cancel(spec.request_id, spec.device_code)
        .await
        .unwrap();
    assert!(second.submit(&other).await.is_err());
    first.close().await.unwrap();
    assert!(second.submit(&other).await.is_err());
    first
        .finish(spec.request_id, "cancelled", None)
        .await
        .unwrap();
    assert!(second.submit(&other).await.unwrap().1);
}

#[tokio::test]
async fn stable_cursor_does_not_repeat_entries_after_new_submissions() {
    let dir = tempfile::tempdir().unwrap();
    let q = queue(&dir.path().join("db"), "first").await;
    for _ in 0..4 {
        q.submit(&request()).await.unwrap();
    }
    let page = q.operation_page(None, None, None, 2).await.unwrap();
    let last = &page[1];
    let time = last["started_at_unix_ms"].as_i64().unwrap();
    let id = last["operation_id"].as_str().unwrap();
    q.submit(&request()).await.unwrap();
    let next = q
        .operation_page(None, None, Some((time, id)), 10)
        .await
        .unwrap();
    assert_eq!(next.len(), 2);
    assert!(
        next.iter()
            .all(|r| page.iter().all(|p| p["operation_id"] != r["operation_id"]))
    );
}

#[tokio::test]
async fn download_completion_requires_local_published_file_evidence() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let q = queue(&dir.path().join("db"), "first").await;
    let mut spec = request();
    spec.direction = "download".into();
    spec.source = "/tmp/source.bin".into();
    spec.destination = dir.path().join("result.bin").to_string_lossy().into_owned();
    q.submit(&spec).await.unwrap();
    let mut snapshot = remote(&spec, "completed", None);
    snapshot.direction = "send".into();
    snapshot.path = spec.source.clone();
    snapshot.size = 4;
    snapshot.offset = 4;
    snapshot.sha256 = Some(format!("{:x}", Sha256::digest(b"data")));
    let record = q.get(spec.request_id, spec.device_code).await.unwrap();
    q.reconcile(&record, &snapshot).await.unwrap();
    assert!(
        q.get(spec.request_id, spec.device_code)
            .await
            .unwrap()
            .operation
            .finished_at_unix_ms
            .is_none()
    );
    tokio::fs::write(&spec.destination, b"evil").await.unwrap();
    q.reconcile(&record, &snapshot).await.unwrap();
    assert!(
        q.get(spec.request_id, spec.device_code)
            .await
            .unwrap()
            .operation
            .finished_at_unix_ms
            .is_none()
    );
    tokio::fs::write(&spec.destination, b"data").await.unwrap();
    q.reconcile(&record, &snapshot).await.unwrap();
    assert_eq!(
        q.get(spec.request_id, spec.device_code)
            .await
            .unwrap()
            .operation
            .state,
        "completed"
    );
}

#[tokio::test]
async fn commands_and_transfers_share_listing_but_not_another_sessions_controls() {
    use pab_protocol::{CommandTaskSpec, ExpectedEnvironment, OsFamily};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let first = queue(&path, "first").await;
    let second = queue(&path, "second").await;
    let spec = request();
    first.submit(&spec).await.unwrap();
    first
        .store
        .remember_device(&super::super::RememberedDevice {
            device_ref: spec.device_ref,
            code: spec.device_code,
            alias: "fixture".into(),
            os_family: OsFamily::Linux,
            os_reminder: "Linux".into(),
        })
        .await
        .unwrap();
    let command_id = RequestId::new();
    let command = CommandTaskSpec {
        options: Default::default(),
        program: "printf".into(),
        args: vec!["fixture".into()],
        cwd: None,
        expected_environment: ExpectedEnvironment {
            os_family: OsFamily::Linux,
            environment_revision: "fixture".into(),
        },
        display_summary: "fixture".into(),
    };
    first
        .store
        .record_pending(spec.device_ref, command_id, &command)
        .await
        .unwrap();
    first.track_task(command_id).await.unwrap();
    assert!(first.command(command_id, spec.device_code).await.is_ok());
    assert!(first.owns_task(command_id).await.unwrap());
    assert!(second.command(command_id, spec.device_code).await.is_err());
    assert!(!second.owns_task(command_id).await.unwrap());
    let page = first
        .operation_page(Some(spec.device_code), Some("running"), None, 20)
        .await
        .unwrap();
    assert_eq!(page.len(), 2);
    assert!(page.iter().any(|record| record["kind"] == "command"));
    assert!(page.iter().any(|record| record["kind"] == "file_transfer"));
    assert!(
        second
            .operation_page(None, None, None, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        first
            .command(command_id, "987654321".parse().unwrap())
            .await
            .is_err()
    );
    first.close().await.unwrap();
    assert!(second.command(command_id, spec.device_code).await.is_ok());
    let other_actor = TransferQueue::open(&path, "third".into(), "account:another".into())
        .await
        .unwrap();
    assert!(
        other_actor
            .command(command_id, spec.device_code)
            .await
            .is_err()
    );
}
