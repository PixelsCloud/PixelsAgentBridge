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
}

impl RelayPolicyRuntime {
    #[cfg(debug_assertions)]
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
        }
    }

    pub fn policy_version(&self) -> Result<Option<u64>, PolicyRuntimeError> {
        Ok(self.lock()?.policy_version())
    }

    pub fn apply_snapshot(&self, snapshot: RelayPolicySnapshot) -> Result<u64, PolicyRuntimeError> {
        let now_unix_ms = current_unix_millis()?;
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
        let now_unix_ms = current_unix_millis()?;
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
            .endpoint_owner(endpoint_key(endpoint_id), current_unix_millis()?))
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, RelayPolicyState>, PolicyRuntimeError> {
        self.state.lock().map_err(|_| PolicyRuntimeError::Poisoned)
    }
}

impl AccessControl for RelayPolicyRuntime {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
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
    fn on_forwarded(&self, src: EndpointId, dst: EndpointId, bytes: usize) {
        let Ok(now) = current_unix_millis() else {
            return;
        };
        let Ok(state) = self.lock() else {
            return;
        };
        let scope = state.traffic_scope(endpoint_key(src), endpoint_key(dst), now);
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
        let Ok(now_unix_ms) = current_unix_millis() else {
            return ForwardingDecision::Drop;
        };
        let Ok(mut state) = self.lock() else {
            return ForwardingDecision::Drop;
        };
        let Some(scope) = state.traffic_scope(endpoint_key(src), endpoint_key(dst), now_unix_ms)
        else {
            // A new pair grant may be newer than this snapshot. Keep failing
            // closed until the authenticated control channel supplies it; QUIC
            // retransmission can then finish the same connection attempt.
            self.refresh_requested.notify_one();
            return ForwardingDecision::Drop;
        };
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
