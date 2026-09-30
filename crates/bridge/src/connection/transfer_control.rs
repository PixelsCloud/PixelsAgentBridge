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
}

impl TransferControl {
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
