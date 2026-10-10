use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use pab_protocol::{
    DeviceId, EndpointKey, RelayEndpointOwner, RelayPolicySnapshot, TrafficScope,
    mbps_to_bytes_per_second,
};
use thiserror::Error;

use crate::{Acquire, AggregateLimiter, LimitKey, Rate};

#[derive(Debug)]
pub struct RelayPolicyState {
    burst: Duration,
    policy_version: Option<u64>,
    expires_at_unix_ms: i64,
    endpoints: HashMap<EndpointKey, RelayEndpointOwner>,
    connection_intents: HashMap<(EndpointKey, DeviceId), i64>,
    limiter: AggregateLimiter,
}

impl RelayPolicyState {
    pub fn new(burst: Duration) -> Result<Self, PolicyStateError> {
        if burst.is_zero() {
            return Err(PolicyStateError::ZeroBurst);
        }
        Ok(Self {
            burst,
            policy_version: None,
            expires_at_unix_ms: 0,
            endpoints: HashMap::new(),
            connection_intents: HashMap::new(),
            limiter: AggregateLimiter::default(),
        })
    }

    pub const fn policy_version(&self) -> Option<u64> {
        self.policy_version
    }

    pub fn apply_snapshot(
        &mut self,
        snapshot: RelayPolicySnapshot,
        now_unix_ms: i64,
        now: Instant,
    ) -> Result<(), PolicyStateError> {
        snapshot
            .validate()
            .map_err(|error| PolicyStateError::InvalidSnapshot(error.to_string()))?;
        if snapshot.issued_at_unix_ms > now_unix_ms || snapshot.expires_at_unix_ms <= now_unix_ms {
            return Err(PolicyStateError::SnapshotNotCurrent);
        }
        if self
            .policy_version
            .is_some_and(|version| snapshot.policy_version <= version)
        {
            return Err(PolicyStateError::StalePolicyVersion);
        }

        let user_limits = snapshot
            .user_limits
            .iter()
            .map(|limit| (limit.user_id, limit.mbps))
            .collect::<HashMap<_, _>>();
        let mut rates = HashMap::new();

        let mut endpoints = HashMap::with_capacity(snapshot.endpoints.len());
        for endpoint in &snapshot.endpoints {
            // Process registration alone never grants Relay access.
            if matches!(
                endpoint.owner,
                RelayEndpointOwner::Guest
                    | RelayEndpointOwner::User {
                        scope: TrafficScope::Guest { .. }
                    }
            ) {
                continue;
            }
            endpoints.insert(endpoint.endpoint_key, endpoint.owner);
            match endpoint.owner {
                RelayEndpointOwner::Device { .. } => {}
                RelayEndpointOwner::Guest => {}
                RelayEndpointOwner::User { scope } => match scope {
                    TrafficScope::User { user_id } => {
                        let mbps = user_limits
                            .get(&user_id)
                            .copied()
                            .unwrap_or(snapshot.defaults.user_mbps);
                        rates.insert(LimitKey::User(user_id), rate(mbps, self.burst)?);
                    }
                    TrafficScope::Guest { .. } => return Err(PolicyStateError::InvalidRate),
                },
            }
        }

        let retained = rates.keys().copied().collect::<HashSet<_>>();
        for (key, rate) in rates {
            self.limiter.set_rate(key, rate, now);
        }
        self.limiter.retain(now, |key| retained.contains(&key));
        self.endpoints = endpoints;
        self.connection_intents = snapshot
            .connection_intents
            .iter()
            .map(|grant| {
                (
                    (grant.operator_endpoint_key, grant.device_id),
                    grant.expires_at_unix_ms,
                )
            })
            .collect();
        self.policy_version = Some(snapshot.policy_version);
        self.expires_at_unix_ms = snapshot.expires_at_unix_ms;
        Ok(())
    }

    pub fn refresh_expiry(
        &mut self,
        policy_version: u64,
        expires_at_unix_ms: i64,
        now_unix_ms: i64,
    ) -> Result<(), PolicyStateError> {
        if self.policy_version != Some(policy_version) {
            return Err(PolicyStateError::UnexpectedPolicyVersion);
        }
        if expires_at_unix_ms <= now_unix_ms {
            return Err(PolicyStateError::SnapshotNotCurrent);
        }
        self.expires_at_unix_ms = self.expires_at_unix_ms.max(expires_at_unix_ms);
        Ok(())
    }

    pub fn endpoint_owner(
        &self,
        endpoint_key: EndpointKey,
        now_unix_ms: i64,
    ) -> Option<RelayEndpointOwner> {
        self.is_current(now_unix_ms)
            .then(|| self.endpoints.get(&endpoint_key).copied())
            .flatten()
    }

    pub fn traffic_scope(
        &self,
        first: EndpointKey,
        second: EndpointKey,
        now_unix_ms: i64,
    ) -> Option<TrafficScope> {
        let first_key = first;
        let second_key = second;
        let first = self.endpoint_owner(first_key, now_unix_ms)?;
        let second = self.endpoint_owner(second_key, now_unix_ms)?;
        match (first, second) {
            (RelayEndpointOwner::User { scope }, RelayEndpointOwner::Device { device_id, .. })
                if self.has_intent(first_key, device_id, now_unix_ms) =>
            {
                Some(scope)
            }
            (RelayEndpointOwner::Device { device_id, .. }, RelayEndpointOwner::User { scope })
                if self.has_intent(second_key, device_id, now_unix_ms) =>
            {
                Some(scope)
            }
            _ => None,
        }
    }

    fn has_intent(&self, operator: EndpointKey, device_id: DeviceId, now_unix_ms: i64) -> bool {
        self.connection_intents
            .get(&(operator, device_id))
            .is_some_and(|expires| *expires > now_unix_ms)
    }

    pub fn acquire(&mut self, scope: TrafficScope, bytes: u64, now: Instant) -> Acquire {
        self.limiter.acquire(scope, bytes, now)
    }

    pub fn is_current(&self, now_unix_ms: i64) -> bool {
        self.policy_version.is_some() && now_unix_ms < self.expires_at_unix_ms
    }
}

fn rate(mbps: u32, burst: Duration) -> Result<Rate, PolicyStateError> {
    Rate::new(mbps_to_bytes_per_second(mbps), burst).ok_or(PolicyStateError::InvalidRate)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PolicyStateError {
    #[error("Relay limiter burst must be greater than zero")]
    ZeroBurst,
    #[error("Relay policy snapshot is invalid: {0}")]
    InvalidSnapshot(String),
    #[error("Relay policy is not valid at the current time")]
    SnapshotNotCurrent,
    #[error("Relay policy version is not newer than the applied version")]
    StalePolicyVersion,
    #[error("Relay policy refresh does not match the applied version")]
    UnexpectedPolicyVersion,
    #[error("Relay policy contains an invalid rate")]
    InvalidRate,
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
