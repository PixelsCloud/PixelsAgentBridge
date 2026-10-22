use crate::{Acquire, AggregateLimiter, LimitKey, Rate};
use pab_protocol::{
    MAX_RELAY_BUDGET_DEMANDS, RELAY_CLUSTER_SCHEMA_VERSION, RelayBudgetSnapshot, UserId,
};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

#[derive(Debug)]
pub(crate) struct RelayBudgets {
    node_id: String,
    boot_id: uuid::Uuid,
    revision: u64,
    leases: HashMap<UserId, (u64, Instant)>,
    demands: HashMap<UserId, Instant>,
    limiter: AggregateLimiter,
    host_bandwidth: bool,
}
impl RelayBudgets {
    pub fn acquire_node(&mut self, bytes: u64, now: Instant) -> Acquire {
        if self.host_bandwidth {
            return Acquire::Ready;
        }
        self.limiter
            .acquire_keys(&[LimitKey::RelayNode], bytes, now)
    }
    pub fn new(node_id: String, boot_id: uuid::Uuid) -> Self {
        Self {
            node_id,
            boot_id,
            revision: 0,
            leases: HashMap::new(),
            demands: HashMap::new(),
            limiter: AggregateLimiter::default(),
            host_bandwidth: false,
        }
    }
    pub fn demand(&mut self, user: UserId, now: Instant) -> bool {
        self.demands
            .retain(|_, seen| now.saturating_duration_since(*seen) < Duration::from_secs(20));
        if !self.demands.contains_key(&user) && self.demands.len() >= MAX_RELAY_BUDGET_DEMANDS {
            return false;
        }
        self.demands.insert(user, now);
        true
    }
    pub fn set_node_capacity(&mut self, bytes_per_second: u64, now: Instant) {
        self.host_bandwidth = bytes_per_second == 0;
        if let Some(rate) = Rate::with_capacity(bytes_per_second, (bytes_per_second / 10).max(1)) {
            self.limiter.set_rate(LimitKey::RelayNode, rate, now);
        }
    }
    pub fn users(&mut self) -> Vec<UserId> {
        let now = Instant::now();
        self.demands
            .retain(|_, seen| now.saturating_duration_since(*seen) < Duration::from_secs(20));
        let mut users: Vec<_> = self.demands.keys().copied().collect();
        users.sort_by_key(|user| user.to_string());
        users
    }
    pub fn acquire(&mut self, user: UserId, bytes: u64, now: Instant) -> Acquire {
        if !self.demand(user, now) {
            return Acquire::Missing(LimitKey::User(user));
        }
        if !self
            .leases
            .get(&user)
            .is_some_and(|(_, deadline)| *deadline > now)
        {
            return Acquire::Missing(LimitKey::User(user));
        }
        let keys = [LimitKey::User(user), LimitKey::RelayNode];
        self.limiter
            .acquire_keys(&keys[..if self.host_bandwidth { 1 } else { 2 }], bytes, now)
    }
    /// Bound validity by REQUEST START, not response receipt or the Relay's
    /// wall clock. Network/DB delay can only shorten a lease, never extend it.
    pub fn apply(
        &mut self,
        snapshot: RelayBudgetSnapshot,
        request_started: Instant,
        now: Instant,
    ) -> Result<(), &'static str> {
        if snapshot.schema_version != RELAY_CLUSTER_SCHEMA_VERSION
            || snapshot.node_id != self.node_id
            || snapshot.boot_id != self.boot_id
            || snapshot.revision <= self.revision
            || snapshot.requested_users.len() > MAX_RELAY_BUDGET_DEMANDS
        {
            return Err("invalid or stale Relay budget snapshot");
        }
        let requested: HashSet<_> = snapshot.requested_users.iter().copied().collect();
        let mut seen = HashSet::new();
        let mut validated = Vec::new();
        if requested.len() != snapshot.requested_users.len() {
            return Err("duplicate budget account");
        }
        for grant in &snapshot.grants {
            let validity = grant
                .expires_at_unix_ms
                .checked_sub(snapshot.issued_at_unix_ms)
                .ok_or("invalid budget expiry")?;
            if !requested.contains(&grant.user_id)
                || !seen.insert(grant.user_id)
                || grant.generation == 0
                || grant.bytes_per_second == 0
                || grant.burst_bytes == 0
                || validity > 15_000
                || self
                    .leases
                    .get(&grant.user_id)
                    .is_some_and(|(generation, _)| grant.generation < *generation)
            {
                return Err("invalid Relay budget grant");
            }
            let duration = Duration::from_millis(validity.saturating_sub(250).max(0) as u64);
            let deadline = request_started
                .checked_add(duration)
                .ok_or("invalid budget deadline")?;
            let rate = Rate::with_capacity(grant.bytes_per_second, grant.burst_bytes)
                .ok_or("invalid budget rate")?;
            validated.push((grant, deadline, rate));
        }
        self.set_node_capacity(snapshot.node_capacity_bytes_per_second, now);
        for user in &requested {
            self.leases.remove(user);
        }
        for (grant, deadline, rate) in validated {
            if deadline > now {
                self.limiter
                    .set_rate(LimitKey::User(grant.user_id), rate, now);
                self.leases
                    .insert(grant.user_id, (grant.generation, deadline));
            }
        }
        self.leases.retain(|_, (_, deadline)| *deadline > now);
        self.limiter.retain(now, |key| {
            key == LimitKey::RelayNode
                || matches!(key,LimitKey::User(user) if self.leases.contains_key(&user))
        });
        self.revision = snapshot.revision;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_bandwidth_removes_only_node_cap_and_keeps_account_lease_and_limit() {
        let user = UserId::new();
        let boot = uuid::Uuid::new_v4();
        let now = Instant::now();
        let mut state = RelayBudgets::new("a".into(), boot);
        let mut grant = snapshot(user, boot);
        grant.node_capacity_bytes_per_second = 0;
        state.apply(grant, now, now).unwrap();
        assert_eq!(state.acquire_node(1_000_000_000, now), Acquire::Ready);
        assert_eq!(state.acquire(user, 100, now), Acquire::Ready);
        assert!(matches!(state.acquire(user, 1, now), Acquire::Wait(_)));
        assert!(matches!(
            state.acquire(user, 1, now + Duration::from_secs(16)),
            Acquire::Missing(_)
        ));
    }
    fn snapshot(user: UserId, boot: uuid::Uuid) -> RelayBudgetSnapshot {
        RelayBudgetSnapshot {
            schema_version: 1,
            node_id: "a".into(),
            boot_id: boot,
            revision: 1,
            issued_at_unix_ms: 1000,
            node_capacity_bytes_per_second: 10_000,
            requested_users: vec![user],
            grants: vec![pab_protocol::RelayBudgetGrant {
                user_id: user,
                generation: 1,
                bytes_per_second: 1000,
                burst_bytes: 100,
                expires_at_unix_ms: 16000,
            }],
        }
    }
    #[test]
    fn expiry_uses_monotonic_request_start_and_replay_cannot_refill_tokens() {
        let user = UserId::new();
        let boot = uuid::Uuid::new_v4();
        let now = Instant::now();
        let mut state = RelayBudgets::new("a".into(), boot);
        state.set_node_capacity(10_000, now);
        let grant = snapshot(user, boot);
        state
            .apply(grant.clone(), now, now + Duration::from_secs(3))
            .unwrap();
        assert_eq!(
            state.acquire(user, 100, now + Duration::from_secs(3)),
            Acquire::Ready
        );
        assert!(
            state
                .apply(
                    grant,
                    now + Duration::from_secs(3),
                    now + Duration::from_secs(3)
                )
                .is_err()
        );
        assert!(matches!(
            state.acquire(user, 1, now + Duration::from_secs(3)),
            Acquire::Wait(_)
        ));
        assert!(matches!(
            state.acquire(user, 1, now + Duration::from_secs(15)),
            Acquire::Missing(_)
        ));
    }
    #[test]
    fn bad_identity_and_malformed_batches_are_atomic_and_never_allow_traffic() {
        let user = UserId::new();
        let boot = uuid::Uuid::new_v4();
        let now = Instant::now();
        let mut state = RelayBudgets::new("a".into(), boot);
        state.set_node_capacity(10_000, now);
        for kind in 0..5 {
            let mut bad = snapshot(user, boot);
            match kind {
                0 => bad.boot_id = uuid::Uuid::new_v4(),
                1 => bad.node_id = "b".into(),
                2 => bad.grants[0].expires_at_unix_ms = 16001,
                3 => bad.grants.push(bad.grants[0].clone()),
                _ => bad.requested_users.clear(),
            }
            assert!(state.apply(bad, now, now).is_err());
            assert!(matches!(state.acquire(user, 1, now), Acquire::Missing(_)));
        }
        state.apply(snapshot(user, boot), now, now).unwrap();
        let mut revoke = snapshot(user, boot);
        revoke.revision = 2;
        revoke.grants.clear();
        state.apply(revoke, now, now).unwrap();
        assert!(matches!(state.acquire(user, 1, now), Acquire::Missing(_)));
    }
}
