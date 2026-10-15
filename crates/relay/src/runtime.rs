use std::{
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use iroh_base::EndpointId;
use iroh_relay::server::{
    Access, AccessControl, ClientRequest, ForwardingControl, ForwardingDecision,
};
use pab_protocol::{EndpointKey, RelayEndpointOwner, RelayPolicySnapshot};
use thiserror::Error;
use tokio::sync::{Notify, watch};

use crate::{Acquire, PolicyStateError, RelayPolicyState};

#[derive(Debug, Clone)]
pub struct RelayPolicyRuntime {
    pub(crate) usage: Arc<
        Mutex<
            std::collections::HashMap<
                (Option<pab_protocol::UserId>, i64),
                pab_protocol::UsageCounters,
            >,
        >,
    >,
    state: Arc<Mutex<RelayPolicyState>>,
    refresh_requested: Arc<Notify>,
    refresh_completed: watch::Sender<u64>,
    budgets: Arc<Mutex<Option<crate::budgets::RelayBudgets>>>,
    probes: Arc<Mutex<Option<crate::probes::RelayProbes>>>,
    clock: Arc<Mutex<Option<(i64, Instant)>>>,
    cluster_mode: Arc<std::sync::atomic::AtomicBool>,
}

impl RelayPolicyRuntime {
    #[cfg(any(debug_assertions, feature = "test-utils"))]
    #[doc(hidden)]
    pub fn usage_for_testing(
        &self,
        user: Option<pab_protocol::UserId>,
    ) -> pab_protocol::UsageCounters {
        let mut total = pab_protocol::UsageCounters::default();
        for ((owner, _), value) in self.usage.lock().unwrap().iter() {
            if *owner == user {
                total.add(value);
            }
        }
        total
    }
    pub fn new(state: RelayPolicyState) -> Self {
        Self {
            usage: Arc::default(),
            state: Arc::new(Mutex::new(state)),
            refresh_requested: Arc::new(Notify::new()),
            refresh_completed: watch::channel(0).0,
            budgets: Arc::new(Mutex::new(None)),
            probes: Arc::new(Mutex::new(None)),
            clock: Arc::default(),
            cluster_mode: Arc::default(),
        }
    }

    pub(crate) fn enable_cluster(&self, node_id: String, boot_id: uuid::Uuid) {
        self.cluster_mode
            .store(true, std::sync::atomic::Ordering::Relaxed);
        *self.probes.lock().expect("probe lock") =
            Some(crate::probes::RelayProbes::new(node_id.clone(), boot_id));
        *self.budgets.lock().expect("budget lock") =
            Some(crate::budgets::RelayBudgets::new(node_id, boot_id));
    }

    pub(crate) fn applied_probe(&self) -> Option<uuid::Uuid> {
        self.probes
            .lock()
            .ok()
            .and_then(|p| p.as_ref().and_then(|p| p.applied()))
    }
    pub(crate) fn apply_probe(
        &self,
        snapshot: pab_protocol::RelayProbeSnapshot,
        started: Instant,
    ) -> Result<(), &'static str> {
        self.probes
            .lock()
            .map_err(|_| "probe lock unavailable")?
            .as_mut()
            .ok_or("cluster probe disabled")?
            .apply(snapshot, started, Instant::now())
    }
    fn probe_decision(
        &self,
        src: EndpointId,
        dst: EndpointId,
        bytes: usize,
    ) -> Option<ForwardingDecision> {
        let Ok(mut guard) = self.probes.lock() else {
            return Some(ForwardingDecision::Drop);
        };
        let probes = guard.as_mut()?;
        if !probes.available(endpoint_key(src), endpoint_key(dst), bytes as u64)? {
            return Some(ForwardingDecision::Drop);
        }
        let Ok(mut budgets) = self.budgets.lock() else {
            return Some(ForwardingDecision::Drop);
        };
        let Some(budgets) = budgets.as_mut() else {
            return Some(ForwardingDecision::Drop);
        };
        match budgets.acquire_node(bytes as u64, Instant::now()) {
            Acquire::Ready => {
                probes.consume(bytes as u64);
                Some(ForwardingDecision::Allow)
            }
            Acquire::Wait(wait) => Some(ForwardingDecision::Wait(wait)),
            _ => Some(ForwardingDecision::Drop),
        }
    }

    pub(crate) fn budget_users(&self) -> Vec<pab_protocol::UserId> {
        self.budgets
            .lock()
            .ok()
            .and_then(|mut state| state.as_mut().map(|state| state.users()))
            .unwrap_or_default()
    }

    pub(crate) fn set_node_capacity(&self, bytes_per_second: u64) {
        if let Ok(mut budgets) = self.budgets.lock() {
            if let Some(state) = budgets.as_mut() {
                state.set_node_capacity(bytes_per_second, Instant::now());
            }
        }
    }

    pub(crate) fn apply_budgets(
        &self,
        snapshot: pab_protocol::RelayBudgetSnapshot,
        started: Instant,
    ) -> Result<(), &'static str> {
        self.budgets
            .lock()
            .map_err(|_| "budget state unavailable")?
            .as_mut()
            .ok_or("cluster budget mode is disabled")?
            .apply(snapshot, started, Instant::now())
    }

    pub fn policy_version(&self) -> Result<Option<u64>, PolicyRuntimeError> {
        Ok(self.lock()?.policy_version())
    }

    fn policy_now(&self) -> Result<i64, PolicyRuntimeError> {
        if let Some((server, instant)) = *self
            .clock
            .lock()
            .map_err(|_| PolicyRuntimeError::Poisoned)?
        {
            return server
                .checked_add(
                    i64::try_from(instant.elapsed().as_millis())
                        .map_err(|_| PolicyRuntimeError::InvalidSystemClock)?,
                )
                .ok_or(PolicyRuntimeError::InvalidSystemClock);
        }
        current_unix_millis()
    }
    fn response_clock(issued: i64, started: Instant) -> Result<(i64, Instant), PolicyRuntimeError> {
        let now = Instant::now();
        // Charging the complete RPC duration (including time before issue)
        // makes expiration conservative. A delayed response never extends TTL.
        let server = issued
            .checked_add(
                i64::try_from(now.saturating_duration_since(started).as_millis())
                    .map_err(|_| PolicyRuntimeError::InvalidSystemClock)?,
            )
            .ok_or(PolicyRuntimeError::InvalidSystemClock)?;
        Ok((server, now))
    }
    pub(crate) fn apply_control_snapshot(
        &self,
        snapshot: RelayPolicySnapshot,
        started: Instant,
    ) -> Result<u64, PolicyRuntimeError> {
        if !self.cluster_mode.load(std::sync::atomic::Ordering::Relaxed) {
            return self.apply_snapshot(snapshot);
        }
        let anchor = Self::response_clock(snapshot.issued_at_unix_ms, started)?;
        let version = snapshot.policy_version;
        let mut state = self.lock()?;
        state.apply_snapshot(snapshot, anchor.0, anchor.1)?;
        *self
            .clock
            .lock()
            .map_err(|_| PolicyRuntimeError::Poisoned)? = Some(anchor);
        self.refresh_completed
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        Ok(version)
    }
    pub(crate) fn refresh_control_expiry(
        &self,
        version: u64,
        expires: i64,
        issued: Option<i64>,
        started: Instant,
    ) -> Result<(), PolicyRuntimeError> {
        if !self.cluster_mode.load(std::sync::atomic::Ordering::Relaxed) {
            return self.refresh_expiry(version, expires);
        }
        let anchor = Self::response_clock(
            issued.ok_or(PolicyRuntimeError::InvalidSystemClock)?,
            started,
        )?;
        let mut state = self.lock()?;
        state.refresh_cluster_expiry(version, expires, anchor.0)?;
        *self
            .clock
            .lock()
            .map_err(|_| PolicyRuntimeError::Poisoned)? = Some(anchor);
        self.refresh_completed
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        Ok(())
    }

    pub fn apply_snapshot(&self, snapshot: RelayPolicySnapshot) -> Result<u64, PolicyRuntimeError> {
        let now_unix_ms = self.policy_now()?;
        let version = snapshot.policy_version;
        self.lock()?
            .apply_snapshot(snapshot, now_unix_ms, Instant::now())?;
        self.refresh_completed
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        Ok(version)
    }

    pub fn refresh_expiry(
        &self,
        policy_version: u64,
        expires_at_unix_ms: i64,
    ) -> Result<(), PolicyRuntimeError> {
        let now_unix_ms = self.policy_now()?;
        self.lock()?
            .refresh_expiry(policy_version, expires_at_unix_ms, now_unix_ms)?;
        self.refresh_completed
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        Ok(())
    }

    pub(crate) async fn wait_for_refresh_request(&self) {
        self.refresh_requested.notified().await;
    }

    pub fn endpoint_owner(
        &self,
        endpoint_id: EndpointId,
    ) -> Result<Option<RelayEndpointOwner>, PolicyRuntimeError> {
        Ok(self
            .lock()?
            .endpoint_owner(endpoint_key(endpoint_id), self.policy_now()?))
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, RelayPolicyState>, PolicyRuntimeError> {
        self.state.lock().map_err(|_| PolicyRuntimeError::Poisoned)
    }
}

impl AccessControl for RelayPolicyRuntime {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        let probe = self.probes.lock().ok().is_some_and(|p| {
            p.as_ref()
                .is_some_and(|p| p.admits(endpoint_key(request.endpoint_id())))
        });
        if probe
            && self
                .lock()
                .is_ok_and(|state| self.policy_now().is_ok_and(|now| state.is_current(now)))
        {
            return Access::Allow;
        }
        // Registration and address lookup commit before a client uses Relay,
        // but the periodic snapshot may still predate that commit. Refresh and
        // recheck instead of rejecting a newly registered endpoint for 20 s.
        let mut completed = self.refresh_completed.subscribe();
        if matches!(self.endpoint_owner(request.endpoint_id()), Ok(None)) {
            self.refresh_requested.notify_one();
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(3), completed.changed()).await;
        }
        match self.endpoint_owner(request.endpoint_id()) {
            Ok(Some(_)) => Access::Allow,
            Ok(None) | Err(_) => Access::Deny {
                reason: Some("endpoint is not admitted by a current policy".to_owned()),
            },
        }
    }
}

impl ForwardingControl for RelayPolicyRuntime {
    fn split_datagram_batches(&self) -> bool {
        self.budgets.lock().is_ok_and(|state| state.is_some())
    }
    fn on_forwarded(&self, src: EndpointId, dst: EndpointId, bytes: usize) {
        let Ok(now) = self.policy_now() else {
            return;
        };
        let Ok(state) = self.lock() else {
            return;
        };
        let scope = state.traffic_scope(endpoint_key(src), endpoint_key(dst), now);
        if scope.is_none()
            && self.probes.lock().ok().is_some_and(|p| {
                p.as_ref()
                    .is_some_and(|p| p.was_probe(endpoint_key(src), endpoint_key(dst)))
            })
        {
            return;
        }
        let user = scope.and_then(|v| v.user_id());
        let upload = !matches!(
            state.endpoint_owner(endpoint_key(src), now),
            Some(RelayEndpointOwner::Device { .. })
        );
        drop(state);
        let Ok(mut usage) = self.usage.lock() else {
            return;
        };
        let key = (user, now - now % 3_600_000);
        if usage.len() >= 10_000 && !usage.contains_key(&key) {
            usage.entry((None, key.1)).or_default().incomplete = true;
            return;
        }
        let value = pab_protocol::UsageCounters {
            relay_upload_bytes: if upload { bytes as u64 } else { 0 },
            relay_download_bytes: if upload { 0 } else { bytes as u64 },
            incomplete: scope.is_none(),
            ..Default::default()
        };
        usage.entry(key).or_default().add(&value);
    }
    fn check(&self, src: EndpointId, dst: EndpointId, bytes: usize) -> ForwardingDecision {
        let Ok(now_unix_ms) = self.policy_now() else {
            return ForwardingDecision::Drop;
        };
        let Ok(mut state) = self.lock() else {
            return ForwardingDecision::Drop;
        };
        if let Some(decision) = self.probe_decision(src, dst, bytes) {
            return if state.is_current(now_unix_ms) {
                decision
            } else {
                ForwardingDecision::Drop
            };
        }
        let Some(scope) = state.traffic_scope(endpoint_key(src), endpoint_key(dst), now_unix_ms)
        else {
            // A new pair grant may be newer than this snapshot. Keep failing
            // closed until the authenticated control channel supplies it; QUIC
            // retransmission can then finish the same connection attempt.
            self.refresh_requested.notify_one();
            return ForwardingDecision::Drop;
        };
        let Ok(mut budgets) = self.budgets.lock() else {
            return ForwardingDecision::Drop;
        };
        if let Some(budgets) = budgets.as_mut() {
            let Some(user) = scope.user_id() else {
                return ForwardingDecision::Drop;
            };
            match budgets.acquire(user, bytes as u64, Instant::now()) {
                Acquire::Ready => {}
                Acquire::Wait(duration) => return ForwardingDecision::Wait(duration),
                _ => return ForwardingDecision::Drop,
            }
        }
        match state.acquire(scope, bytes as u64, Instant::now()) {
            Acquire::Ready => ForwardingDecision::Allow,
            Acquire::Wait(duration) => ForwardingDecision::Wait(duration),
            Acquire::Oversized { .. } | Acquire::Missing(_) => ForwardingDecision::Drop,
        }
    }
}

fn endpoint_key(endpoint_id: EndpointId) -> EndpointKey {
    EndpointKey::new(*endpoint_id.as_bytes())
}

fn current_unix_millis() -> Result<i64, PolicyRuntimeError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| PolicyRuntimeError::InvalidSystemClock)?;
    i64::try_from(duration.as_millis()).map_err(|_| PolicyRuntimeError::InvalidSystemClock)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PolicyRuntimeError {
    #[error("Relay policy state lock is poisoned")]
    Poisoned,
    #[error("system clock cannot provide a Unix timestamp")]
    InvalidSystemClock,
    #[error(transparent)]
    Policy(#[from] PolicyStateError),
}
