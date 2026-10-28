use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, Ordering},
};

use super::BridgeError;

const CANCEL: u8 = 1;
const COMMIT: u8 = 2;
const REQUESTED: u8 = 4;
const STAGING_DISCARDED:u8=8;
const DISCARD_CANCELLED:u8=16;
type PublicationGuard = Arc<dyn Fn(pab_file_manager::AttemptEvidence)->std::pin::Pin<Box<dyn std::future::Future<Output=Result<(),BridgeError>>+Send>>+Send+Sync>;

/// The local publication gate makes cancel-before-publish deterministic. Uploads
/// additionally need the Executor's facts: stopping a stream cannot undo a publish.
#[derive(Clone, Default)]
pub struct TransferControl {
    state: Arc<AtomicU8>,
    digest: Arc<Mutex<Option<String>>>,
    accepted: Arc<Mutex<Option<pab_protocol::TransferSnapshot>>>,
    expected_context: Arc<Mutex<Option<pab_protocol::ExecutionContext>>>,
    checkpoint: Arc<Mutex<Option<u64>>>,
    expected_size: Arc<Mutex<Option<u64>>>,
    expected_source_modified:Arc<Mutex<Option<i64>>>,
    local_publication:Arc<Mutex<Option<bool>>>,
    local_metadata_requested:Arc<Mutex<Option<pab_protocol::FileTransferMetadata>>>,
    local_metadata_result:Arc<Mutex<Option<pab_protocol::FileTransferMetadataResult>>>,
    publication_guard:Arc<Mutex<Option<PublicationGuard>>>,
    local_staging:Arc<Mutex<Option<pab_file_manager::PrivateFileStagingReceipt>>>,
    resume_staging:Arc<Mutex<Option<pab_file_manager::PrivateFileStagingReceipt>>>,
    cleanup_error:Arc<Mutex<Option<String>>>,
    #[cfg(test)]
    publication_barrier:Arc<Mutex<Option<super::PublicationBarrier>>>,
}

impl TransferControl {
    pub fn expect_source_modified(&self,modified:Option<i64>){*self.expected_source_modified.lock().unwrap_or_else(|error|error.into_inner())=modified;}
    pub(super) fn check_source_metadata(&self,metadata:&std::fs::Metadata)->Result<(),BridgeError> {
        let expected=self.expected_source_modified.lock().unwrap_or_else(|error|error.into_inner());
        let size=self.expected_size.lock().unwrap_or_else(|error|error.into_inner()).unwrap_or(metadata.len());
        pab_file_manager::verify_opened_source_metadata(metadata,size,*expected)?;Ok(())
    }
    pub(crate) fn set_publication_guard(&self,guard:PublicationGuard){*self.publication_guard.lock().unwrap_or_else(|error|error.into_inner())=Some(guard);}
    #[cfg(unix)]
    pub(super) fn has_publication_guard(&self)->bool {self.publication_guard.lock().unwrap_or_else(|error|error.into_inner()).is_some()}
    pub(crate) fn set_resume_staging(&self,receipt:Option<pab_file_manager::PrivateFileStagingReceipt>){*self.resume_staging.lock().unwrap_or_else(|error|error.into_inner())=receipt;}
    #[cfg(unix)]
    pub(super) fn resume_staging(&self)->Option<pab_file_manager::PrivateFileStagingReceipt>{self.resume_staging.lock().unwrap_or_else(|error|error.into_inner()).clone()}
    #[cfg(unix)]
    pub(super) async fn persist_staging(&self,receipt:pab_file_manager::PrivateFileStagingReceipt)->Result<(),BridgeError>{
        receipt.validate()?;
        {
            let mut current=self.local_staging.lock().unwrap_or_else(|error|error.into_inner());
            if current.as_ref().is_some_and(|old|old!=&receipt){return Err(BridgeError::FileTransfer("download staging ownership changed".into()));}
            *current=Some(receipt);
        }
        self.persist_local_publication().await
    }
    #[cfg(unix)]
    pub(super) async fn persist_staging_discarded(&self)->Result<(),BridgeError>{
        if self.committing() || self.local_staging.lock().unwrap_or_else(|error|error.into_inner()).is_none(){return Err(BridgeError::FileTransfer("download staging removal is not owned".into()));}
        self.state.fetch_or(STAGING_DISCARDED,Ordering::AcqRel);
        self.persist_local_publication().await
    }
    pub(super) async fn persist_publication_intent(&self)->Result<(),BridgeError> {
        let guard=self.publication_guard.lock().unwrap_or_else(|error|error.into_inner()).clone();
        if let Some(guard)=guard {let mut evidence=self.evidence();evidence.local_publication_intent=true;guard(evidence).await?;}
        Ok(())
    }
    pub(crate) fn evidence(&self)->pab_file_manager::AttemptEvidence {
        let receipt=self.accepted();
        pab_file_manager::AttemptEvidence {local_staging:self.local_staging.lock().unwrap_or_else(|error|error.into_inner()).clone(),local_staging_discarded:self.state.load(Ordering::Acquire)&STAGING_DISCARDED!=0,digest:self.digest(),context:receipt.as_ref().and_then(|snapshot|snapshot.execution_context.clone()),receipt,local_checkpoint:self.durable_checkpoint().map(pab_protocol::FileByteCount),local_publication_intent:false,local_published:self.local_publication()==Some(true),local_metadata_requested:self.local_metadata_requested.lock().unwrap_or_else(|e|e.into_inner()).clone(),local_metadata_result:self.local_metadata_result()}
    }
    pub(super) fn expect_local_metadata(&self,metadata:Option<pab_protocol::FileTransferMetadata>)->Result<(),BridgeError> {
        if let Some(value)=&metadata {value.validate().map_err(|error|BridgeError::FileTransfer(error.into()))?;}
        let mut expected=self.local_metadata_requested.lock().unwrap_or_else(|e|e.into_inner());
        if expected.as_ref().is_some_and(|previous|Some(previous)!=metadata.as_ref()) || self.committing() {return Err(BridgeError::FileTransfer("download attribute request changed".into()));}
        *expected=metadata;Ok(())
    }
    pub fn local_metadata_result(&self)->Option<pab_protocol::FileTransferMetadataResult> {self.local_metadata_result.lock().unwrap_or_else(|e|e.into_inner()).clone()}
    pub(crate) fn local_metadata_pending(&self)->bool {self.local_metadata_requested.lock().unwrap_or_else(|e|e.into_inner()).is_some() && self.local_metadata_result().is_none()}
    pub(super) fn record_local_metadata(&self,result:pab_protocol::FileTransferMetadataResult)->Result<(),BridgeError> {
        let expected=self.local_metadata_requested.lock().unwrap_or_else(|e|e.into_inner());
        result.validate_for(expected.as_ref().ok_or_else(||BridgeError::FileTransfer("download attributes were not requested".into()))?).map_err(|error|BridgeError::FileTransfer(error.into()))?;
        if self.local_publication()!=Some(true) {return Err(BridgeError::FileTransfer("download is not published".into()));}
        let mut actual=self.local_metadata_result.lock().unwrap_or_else(|e|e.into_inner());
        if actual.as_ref().is_some_and(|previous|previous!=&result) {return Err(BridgeError::FileTransfer("download attribute result changed".into()));}
        *actual=Some(result);Ok(())
    }
    pub(super) async fn persist_local_publication(&self)->Result<(),BridgeError> {
        let guard=self.publication_guard.lock().unwrap_or_else(|error|error.into_inner()).clone();
        if let Some(guard)=guard {guard(self.evidence()).await?;}Ok(())
    }
    pub fn local_publication(&self)->Option<bool>{*self.local_publication.lock().unwrap_or_else(|error|error.into_inner())}
    pub(super) fn record_local_publication(&self,published:bool){*self.local_publication.lock().unwrap_or_else(|error|error.into_inner())=Some(published);}
    #[cfg(test)]
    pub(crate) fn set_publication_barrier(&self,barrier:super::PublicationBarrier){*self.publication_barrier.lock().unwrap()=Some(barrier);}
    #[cfg(test)]
    pub(super) async fn before_publication(&self) {let barrier={self.publication_barrier.lock().unwrap().clone()};if let Some(barrier)=barrier {barrier.enter().await;}}
    pub fn expect_size(&self,size:u64) {*self.expected_size.lock().unwrap_or_else(|e|e.into_inner())=Some(size);}
    pub(super) fn size_matches(&self,size:u64)->bool {self.expected_size.lock().unwrap_or_else(|e|e.into_inner()).is_none_or(|expected|expected==size)}
    pub(crate) fn observe_snapshot(&self,snapshot:pab_protocol::TransferSnapshot)->Result<(),BridgeError> {self.accept(snapshot)}
    pub fn durable_checkpoint(&self)->Option<u64> {*self.checkpoint.lock().unwrap_or_else(|e|e.into_inner())}
    pub(crate) fn staging_discarded(&self)->bool {self.state.load(Ordering::Acquire)&STAGING_DISCARDED!=0}
    pub(super) fn set_checkpoint(&self,offset:u64) {*self.checkpoint.lock().unwrap_or_else(|e|e.into_inner())=Some(offset);}
    pub fn expect_context(&self, context: pab_protocol::ExecutionContext) {
        *self
            .expected_context
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(context);
    }
    pub(super) fn accept(
        &self,
        snapshot: pab_protocol::TransferSnapshot,
    ) -> Result<(), BridgeError> {
        if self
            .expected_context
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|expected| match pab_protocol::FileExecutionIdentity::from_context(expected) {
                Ok(identity)=>snapshot.execution_context.as_ref().is_none_or(|actual|!identity.matches_context(actual)),
                Err(_)=>snapshot.execution_context.as_ref()!=Some(expected),
            })
        {
            return Err(BridgeError::FileTransfer(
                "transfer execution identity changed".into(),
            ));
        }
        snapshot.validate_publication_facts().map_err(|error|BridgeError::FileTransfer(error.into()))?;
        let mut accepted=self.accepted.lock().unwrap_or_else(|e|e.into_inner());
        if let Some(previous)=accepted.as_ref() {
            if !snapshot.supersedes(previous).map_err(|error|BridgeError::FileTransfer(error.into()))? {return Ok(());}
        }
        *accepted = Some(snapshot);
        Ok(())
    }
    pub fn accepted(&self) -> Option<pab_protocol::TransferSnapshot> {
        self.accepted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn cancel(&self) {
        self.state.fetch_or(CANCEL, Ordering::AcqRel);
        #[cfg(test)]
        if let Some(barrier)=self.publication_barrier.lock().unwrap().as_ref(){barrier.signal_cancel();}
    }
    /// Explicit user cancellation discards receiving data. Pause, account
    /// revocation and connection loss continue to preserve their checkpoints.
    pub(crate) fn cancel_and_discard(&self) {
        self.state.fetch_or(CANCEL|DISCARD_CANCELLED,Ordering::AcqRel);
        self.cancel();
    }
    pub(super) fn discard_cancelled_staging(&self)->bool {
        let state=self.state.load(Ordering::Acquire);
        state&DISCARD_CANCELLED!=0 && state&COMMIT==0
    }
    pub(super) fn record_cancelled_cleanup(&self,result:std::io::Result<()>) {
        match result {
            Ok(())=>{self.state.fetch_or(STAGING_DISCARDED,Ordering::AcqRel);},
            Err(error)=>{*self.cleanup_error.lock().unwrap_or_else(|e|e.into_inner())=Some(error.to_string());},
        }
    }
    pub(crate) fn cleanup_error(&self)->Option<String> {self.cleanup_error.lock().unwrap_or_else(|e|e.into_inner()).clone()}
    pub fn cancelled(&self) -> bool {
        self.state.load(Ordering::Acquire) & CANCEL != 0
    }
    pub fn committing(&self) -> bool {
        self.state.load(Ordering::Acquire) & COMMIT != 0
    }
    pub fn remote_requested(&self) -> bool {
        self.state.load(Ordering::Acquire) & REQUESTED != 0
    }
    pub(super) fn request(&self) -> Result<(), BridgeError> {
        self.check()?;
        self.state.fetch_or(REQUESTED, Ordering::AcqRel);
        self.check()
    }
    pub(super) fn rejected(&self) {
        self.state.fetch_and(!REQUESTED, Ordering::AcqRel);
    }
    pub(super) fn check(&self) -> Result<(), BridgeError> {
        if self.cancelled() {
            Err(BridgeError::FileTransfer(
                "transfer cancellation requested".to_owned(),
            ))
        } else {
            Ok(())
        }
    }
    pub(super) fn commit(&self) -> Result<(), BridgeError> {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            if state & CANCEL != 0 {
                return self.check();
            }
            match self.state.compare_exchange_weak(
                state,
                state | COMMIT,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(current) => state = current,
            }
        }
    }
    pub(super) fn set_digest(&self, digest: &str) {
        *self.digest.lock().unwrap_or_else(|e| e.into_inner()) = Some(digest.to_owned());
    }
    pub fn digest(&self) -> Option<String> {
        self.digest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_keeps_confirmed_attributes_and_rejects_changed_terminal_receipts() {
        use pab_protocol::*;
        let control=TransferControl::default();
        let pending=TransferSnapshot {request_id:RequestId::new(),initiated_by:OperatorRef::account(UserId::from_u128(1),EndpointKey::new([1;32])),initiating_user:None,execution_context:None,direction:"receive".into(),path:"/fixture".into(),state:"committing".into(),offset:10,size:10,sha256:Some("a".repeat(64)),finished_at_unix_ms:None,message:None,published:Some(true),metadata_result:None};
        control.accept(pending.clone()).unwrap();
        let mut applied=pending.clone();
        let metadata=FileTransferMetadata {modified_at_unix_ms:Some(123),unix_mode:None};
        applied.metadata_result=Some(FileTransferMetadataResult {requested:metadata.clone(),actual:metadata,warnings:vec![]});
        control.accept(applied.clone()).unwrap();
        control.accept(pending.clone()).unwrap();
        assert_eq!(control.accepted(),Some(applied.clone()));
        let mut done=applied;done.state="completed".into();done.finished_at_unix_ms=Some(456);
        control.accept(done.clone()).unwrap();
        control.accept(pending).unwrap();
        let mut changed=done.clone();changed.finished_at_unix_ms=Some(457);
        assert!(control.accept(changed).is_err());
        assert_eq!(control.accepted(),Some(done));
    }

    #[tokio::test]
    async fn shared_runner_preserves_unknown_publication_and_cancel_facts() {
        use crate::{TransferQueue, TransferRequest};
        use pab_protocol::{DeviceId, DeviceRef, RequestId, TenantId};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runner.db");
        let queue = TransferQueue::open(&path, "runner".into(), "guest".into())
            .await
            .unwrap();
        let spec = TransferRequest {
            request_id: RequestId::new(),
            device_ref: DeviceRef {
                tenant_id: TenantId::from_u128(2),
                device_id: DeviceId::from_u128(3),
            },
            device_code: "123456789".parse().unwrap(),
            direction: "upload".into(),
            source: dir.path().join("input").to_string_lossy().into_owned(),
            destination: "/remote/file".into(),
            overwrite: false,
            options: Default::default(),
        };
        for (remote_requested, cancel, expected) in [
            (false, false, "failed"),
            (false, true, "cancelled"),
            (true, false, "unconfirmed"),
            (true, true, "unconfirmed"),
        ] {
            let mut spec = spec.clone();
            spec.request_id = RequestId::new();
            queue.submit(&spec).await.unwrap();
            let control = TransferControl::default();
            if remote_requested {
                control.request().unwrap();
            }
            if cancel {
                control.cancel();
            }
            let (_tx, rx) = tokio::sync::watch::channel(false);
            let unresolved = queue
                .run_transfer(
                    &spec,
                    &control,
                    rx,
                    async { Err("fixture lost response".into()) },
                    |_, _| {},
                )
                .await;
            assert_eq!(unresolved, expected == "unconfirmed");
            let reopened = TransferQueue::open(&path, "runner".into(), "guest".into())
                .await
                .unwrap();
            let record = reopened
                .get(spec.request_id, spec.device_code)
                .await
                .unwrap();
            assert_eq!(record.phase, expected);
            assert_eq!(record.operation.finished_at_unix_ms.is_none(), unresolved);
            assert_eq!(
                record.operation.message.as_deref(),
                Some("fixture lost response")
            );
            assert!(
                !reopened.submit(&spec).await.unwrap().1,
                "observing a lost response never starts another worker"
            );
        }
        // Cancellation while runtime initialization is pending must wake the
        // runner without needing a network connection or aborting its owner.
        let mut waiting = spec.clone();
        waiting.request_id = RequestId::new();
        queue.submit(&waiting).await.unwrap();
        let control = TransferControl::default();
        let (tx, rx) = tokio::sync::watch::channel(false);
        tx.send(true).unwrap();
        let unresolved = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            queue.run_transfer(&waiting, &control, rx, std::future::pending(), |_, _| {}),
        )
        .await
        .unwrap();
        assert!(!unresolved);
        assert_eq!(
            queue
                .get(waiting.request_id, waiting.device_code)
                .await
                .unwrap()
                .operation
                .state,
            "cancelled"
        );
    }
    #[test]
    fn cancellation_and_publication_have_a_single_winner() {
        let cancelled = TransferControl::default();
        cancelled.cancel();
        assert!(cancelled.commit().is_err());
        assert!(!cancelled.committing());
        let published = TransferControl::default();
        published.commit().unwrap();
        published.cancel();
        assert!(published.committing());
        assert!(published.cancelled());
    }

    #[test]
    fn racing_cancel_and_publication_never_both_win() {
        for _ in 0..64 {
            let control = TransferControl::default();
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let cancellation = control.clone();
            let ready = barrier.clone();
            let worker = std::thread::spawn(move || {
                ready.wait();
                cancellation.cancel();
            });
            barrier.wait();
            let committed = control.commit().is_ok();
            worker.join().unwrap();
            assert_eq!(control.committing(), committed);
            assert!(control.cancelled());
        }
    }
}
