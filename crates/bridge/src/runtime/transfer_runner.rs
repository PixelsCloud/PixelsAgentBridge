//! Shared durable transfer lifecycle for Desktop and MCP.
use super::{BridgeRuntime, TransferQueue, TransferRequest};
use crate::TransferControl;
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;

impl BridgeRuntime {
    /// Share this runtime's existing owner and heartbeat; do not create another session.
    pub fn transfer_queue(&self) -> TransferQueue {
        TransferQueue {
            store: self.inner.store.clone(),
            session_id: self.inner.session_id.clone(),
            initiated_by: self.inner.initiated_by.clone(),
        }
    }
}

impl TransferQueue {
    /// Run one already accepted submission. True means only observation is safe:
    /// a remote publication or the durable local outcome is not yet confirmed.
    pub async fn run_transfer(
        &self,
        spec: &TransferRequest,
        control: &TransferControl,
        mut cancelled: watch::Receiver<bool>,
        runtime: impl Future<Output = Result<Arc<BridgeRuntime>, String>>,
        emit_progress: impl Fn(u64, u64) + Send + Sync,
    ) -> bool {
        let id = spec.request_id;
        let offset = Arc::new(AtomicU64::new(0));
        let size = Arc::new(AtomicU64::new(0));
        let count = offset.clone();
        let total = size.clone();
        let mut operation = Box::pin(async {
            self.phase(id, "connecting", None)
                .await
                .map_err(|e| e.to_string())?;
            let runtime = runtime.await?;
            runtime
                .execute_queued_transfer(&spec, &control, move |n, all| {
                    count.store(n, Ordering::Release);
                    total.store(all, Ordering::Release);
                })
                .await
                .map_err(|e| e.to_string())
        });
        let mut timer = tokio::time::interval(Duration::from_millis(250));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last = (0, 0);
        let result = loop {
            tokio::select! {
                biased;
                outcome = &mut operation => break outcome,
                _ = cancelled.changed() => {
                    control.cancel();
                    if control.committing() { break operation.as_mut().await; }
                    break Err("transfer cancellation requested".to_owned());
                },
                _ = timer.tick() => {
                    if let Some(snapshot) = control.accepted() {
                        if let Err(e) = self.observe_acceptance(id, spec.device_code, &snapshot).await {
                            break Err(e.to_string());
                        }
                    }
                    let progress = (offset.load(Ordering::Acquire), size.load(Ordering::Acquire));
                    if progress != last {
                        if let Err(e) = self.progress(id, progress.0, progress.1).await { tracing::warn!(%e, "cannot persist transfer progress"); }
                        let phase = if progress.0 == progress.1 { "verifying" } else { "transferring" };
                        let _ = self.phase(id, phase, control.digest().as_deref()).await;
                        emit_progress(progress.0, progress.1);
                        last = progress;
                    }
                }
            }
        };
        // Drop the stream before trying to reconcile cancellation with the Executor.
        drop(operation);
        let result = if let Some(snapshot) = control.accepted() {
            match self
                .observe_acceptance(id, spec.device_code, &snapshot)
                .await
            {
                Ok(()) => result,
                Err(error) => Err(error.to_string()),
            }
        } else {
            result
        };
        let _ = self
            .progress(
                id,
                offset.load(Ordering::Acquire),
                size.load(Ordering::Acquire),
            )
            .await;
        let state = match &result {
            Ok(()) => "completed",
            Err(_)
                if control.remote_requested()
                    && (spec.direction == "upload" || control.committing()) =>
            {
                "unconfirmed"
            }
            Err(_) if control.cancelled() => "cancelled",
            Err(_) => "failed",
        };
        let _ = self.phase(id, state, control.digest().as_deref()).await;
        if let Err(error) = &result
            && let Err(e) = self.note(id, error).await
        {
            tracing::warn!(%e,%id,"cannot persist unresolved transfer error");
        }
        let mut needs_recheck = state == "unconfirmed";
        if !needs_recheck
            && let Err(e) = self
                .finish(id, state, result.as_ref().err().map(String::as_str))
                .await
        {
            tracing::error!(%e, %id, "cannot persist transfer outcome");
            let _ = self
                .phase(id, "unconfirmed", control.digest().as_deref())
                .await;
            needs_recheck = true;
        }
        needs_recheck
    }
}
