use std::{
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use iroh_base::EndpointId;
use iroh_relay::server::{
    Access, AccessControl, ClientRequest, ForwardingControl, ForwardingDecision,
};
use pab_protocol::{DeploymentId, EndpointKey, RelayEndpointOwner, RelayPolicySnapshot};
use thiserror::Error;

use crate::{Acquire, PolicyStateError, RelayPolicyState};

#[derive(Debug, Clone)]
pub struct RelayPolicyRuntime {
    state: Arc<Mutex<RelayPolicyState>>,
}

impl RelayPolicyRuntime {
    pub fn new(state: RelayPolicyState) -> Self {
        Self {
            state: Arc::new(Mutex::new(state)),
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
        Ok(version)
    }

    pub fn refresh_expiry(
        &self,
        deployment_id: DeploymentId,
        policy_version: u64,
        expires_at_unix_ms: i64,
    ) -> Result<(), PolicyRuntimeError> {
        let now_unix_ms = current_unix_millis()?;
        self.lock()?.refresh_expiry(
            deployment_id,
            policy_version,
            expires_at_unix_ms,
            now_unix_ms,
        )?;
        Ok(())
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
        match self.endpoint_owner(request.endpoint_id()) {
            Ok(Some(_)) => Access::Allow,
            Ok(None) | Err(_) => Access::Deny {
                reason: Some("endpoint is not admitted by a current policy".to_owned()),
            },
        }
    }
}

impl ForwardingControl for RelayPolicyRuntime {
    fn check(&self, src: EndpointId, dst: EndpointId, bytes: usize) -> ForwardingDecision {
        let Ok(now_unix_ms) = current_unix_millis() else {
            return ForwardingDecision::Drop;
        };
        let Ok(mut state) = self.lock() else {
            return ForwardingDecision::Drop;
        };
        let Some(scope) = state.traffic_scope(endpoint_key(src), endpoint_key(dst), now_unix_ms)
        else {
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
