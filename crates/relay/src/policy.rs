use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use pab_protocol::{
    DeploymentId, DeviceId, EndpointKey, RelayEndpointOwner, RelayPolicySnapshot, TenantId,
    TrafficScope, mbps_to_bytes_per_second,
};
use thiserror::Error;

use crate::{Acquire, AggregateLimiter, LimitKey, Rate};

#[derive(Debug)]
pub struct RelayPolicyState {
    deployment_id: DeploymentId,
    burst: Duration,
    policy_version: Option<u64>,
    expires_at_unix_ms: i64,
    endpoints: HashMap<EndpointKey, RelayEndpointOwner>,
    guest_grants: HashMap<(EndpointKey, DeviceId), i64>,
    limiter: AggregateLimiter,
}

impl RelayPolicyState {
    pub fn new(deployment_id: DeploymentId, burst: Duration) -> Result<Self, PolicyStateError> {
        if burst.is_zero() {
            return Err(PolicyStateError::ZeroBurst);
        }
        Ok(Self {
            deployment_id,
            burst,
            policy_version: None,
            expires_at_unix_ms: 0,
            endpoints: HashMap::new(),
            guest_grants: HashMap::new(),
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
        if snapshot.deployment_id != self.deployment_id {
            return Err(PolicyStateError::WrongDeployment);
        }
        if snapshot.issued_at_unix_ms > now_unix_ms || snapshot.expires_at_unix_ms <= now_unix_ms {
            return Err(PolicyStateError::SnapshotNotCurrent);
        }
        if self
            .policy_version
            .is_some_and(|version| snapshot.policy_version <= version)
        {
            return Err(PolicyStateError::StalePolicyVersion);
        }

        let team_limits = snapshot
            .team_limits
            .iter()
            .map(|limits| (limits.tenant_id, *limits))
            .collect::<HashMap<_, _>>();
        let mut rates = HashMap::new();
        for limits in &snapshot.team_limits {
            rates.insert(
                LimitKey::Team(limits.tenant_id),
                rate(limits.total_mbps, self.burst)?,
            );
        }

        let mut endpoints = HashMap::with_capacity(snapshot.endpoints.len());
        for endpoint in &snapshot.endpoints {
            endpoints.insert(endpoint.endpoint_key, endpoint.owner);
            match endpoint.owner {
                RelayEndpointOwner::Device { .. } => {}
                RelayEndpointOwner::Guest => {
                    rates.insert(LimitKey::Guest(endpoint.endpoint_key), rate(1, self.burst)?);
                }
                RelayEndpointOwner::User { scope } => match scope {
                    TrafficScope::Team { tenant_id, user_id } => {
                        let limits = team_limits
                            .get(&tenant_id)
                            .ok_or(PolicyStateError::MissingTeamLimits(tenant_id))?;
                        rates.insert(
                            LimitKey::Member { tenant_id, user_id },
                            rate(limits.member_mbps, self.burst)?,
                        );
                    }
                    TrafficScope::Personal { tenant_id, .. } => {
                        rates.insert(
                            LimitKey::Personal(tenant_id),
                            rate(snapshot.defaults.personal_mbps, self.burst)?,
                        );
                    }
                    TrafficScope::Guest { .. } => return Err(PolicyStateError::InvalidRate),
                },
            }
        }

        let retained = rates.keys().copied().collect::<HashSet<_>>();
        for (key, rate) in rates {
            self.limiter.set_rate(key, rate, now);
        }
        self.limiter.retain(|key| retained.contains(&key));
        self.endpoints = endpoints;
        self.guest_grants = snapshot
            .guest_grants
            .iter()
            .map(|grant| {
                (
                    (grant.guest_endpoint_key, grant.device_id),
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
        deployment_id: DeploymentId,
        policy_version: u64,
        expires_at_unix_ms: i64,
        now_unix_ms: i64,
    ) -> Result<(), PolicyStateError> {
        if deployment_id != self.deployment_id {
            return Err(PolicyStateError::WrongDeployment);
        }
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
            (RelayEndpointOwner::User { scope }, RelayEndpointOwner::Device { tenant_id, .. })
            | (RelayEndpointOwner::Device { tenant_id, .. }, RelayEndpointOwner::User { scope })
                if scope.tenant_id() == Some(tenant_id) =>
            {
                Some(scope)
            }
            (RelayEndpointOwner::Guest, RelayEndpointOwner::Device { device_id, .. })
                if self
                    .guest_grants
                    .get(&(first_key, device_id))
                    .is_some_and(|expires| *expires > now_unix_ms) =>
            {
                Some(TrafficScope::Guest {
                    endpoint_key: first_key,
                })
            }
            (RelayEndpointOwner::Device { device_id, .. }, RelayEndpointOwner::Guest)
                if self
                    .guest_grants
                    .get(&(second_key, device_id))
                    .is_some_and(|expires| *expires > now_unix_ms) =>
            {
                Some(TrafficScope::Guest {
                    endpoint_key: second_key,
                })
            }
            _ => None,
        }
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
    #[error("Relay policy belongs to a different deployment")]
    WrongDeployment,
    #[error("Relay policy is not valid at the current time")]
    SnapshotNotCurrent,
    #[error("Relay policy version is not newer than the applied version")]
    StalePolicyVersion,
    #[error("Relay policy refresh does not match the applied version")]
    UnexpectedPolicyVersion,
    #[error("Relay policy has no limits for Team {0}")]
    MissingTeamLimits(TenantId),
    #[error("Relay policy contains an invalid rate")]
    InvalidRate,
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
