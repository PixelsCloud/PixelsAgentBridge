use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, Ordering},
};

use super::BridgeError;

const CANCEL: u8 = 1;
const COMMIT: u8 = 2;
const REQUESTED: u8 = 4;

/// The local publication gate makes cancel-before-publish deterministic. Uploads
/// additionally need the Executor's facts: stopping a stream cannot undo a publish.
#[derive(Clone, Default)]
pub struct TransferControl {
    state: Arc<AtomicU8>,
    digest: Arc<Mutex<Option<String>>>,
    accepted: Arc<Mutex<Option<pab_protocol::TransferSnapshot>>>,
    expected_context: Arc<Mutex<Option<pab_protocol::ExecutionContext>>>,
}

impl TransferControl {
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
            .is_some_and(|expected| snapshot.execution_context.as_ref() != Some(expected))
        {
            return Err(BridgeError::FileTransfer(
                "transfer execution identity changed".into(),
            ));
        }
        *self.accepted.lock().unwrap_or_else(|e| e.into_inner()) = Some(snapshot);
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
    }
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
