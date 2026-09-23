use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use pab_protocol::{EndpointKey, TenantId, TrafficScope, UserId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LimitKey {
    Team(TenantId),
    Member {
        tenant_id: TenantId,
        user_id: UserId,
    },
    Personal(TenantId),
    Guest(EndpointKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    bytes_per_second: u64,
    burst: Duration,
}

impl Rate {
    pub fn new(bytes_per_second: u64, burst: Duration) -> Option<Self> {
        (bytes_per_second > 0 && !burst.is_zero()).then_some(Self {
            bytes_per_second,
            burst,
        })
    }

    fn capacity(self) -> u64 {
        let bytes =
            u128::from(self.bytes_per_second).saturating_mul(self.burst.as_nanos()) / 1_000_000_000;
        bytes.clamp(1, u128::from(u64::MAX)) as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acquire {
    Ready,
    Wait(Duration),
    Oversized { requested: u64, capacity: u64 },
    Missing(LimitKey),
}

#[derive(Debug)]
struct Bucket {
    rate: Rate,
    available: u64,
    remainder: u128,
    updated_at: Instant,
}

impl Bucket {
    fn new(rate: Rate, now: Instant) -> Self {
        Self {
            rate,
            available: rate.capacity(),
            remainder: 0,
            updated_at: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.updated_at);
        self.updated_at = now;
        let numerator = u128::from(self.rate.bytes_per_second)
            .saturating_mul(elapsed.as_nanos())
            .saturating_add(self.remainder);
        let added = numerator / 1_000_000_000;
        self.remainder = numerator % 1_000_000_000;
        self.available = self
            .available
            .saturating_add(added.min(u128::from(u64::MAX)) as u64)
            .min(self.rate.capacity());
    }

    fn wait_for(&self, bytes: u64) -> Duration {
        let missing = bytes.saturating_sub(self.available);
        let nanos = u128::from(missing)
            .saturating_mul(1_000_000_000)
            .div_ceil(u128::from(self.rate.bytes_per_second));
        Duration::from_nanos(nanos.min(u128::from(u64::MAX)) as u64)
    }
}

#[derive(Debug, Default)]
pub struct AggregateLimiter {
    buckets: HashMap<LimitKey, Bucket>,
}

impl AggregateLimiter {
    pub fn set_rate(&mut self, key: LimitKey, rate: Rate, now: Instant) {
        match self.buckets.get_mut(&key) {
            Some(bucket) => {
                bucket.refill(now);
                bucket.rate = rate;
                bucket.available = bucket.available.min(rate.capacity());
            }
            None => {
                self.buckets.insert(key, Bucket::new(rate, now));
            }
        }
    }

    pub fn retain(&mut self, mut keep: impl FnMut(LimitKey) -> bool) {
        self.buckets.retain(|key, _| keep(*key));
    }

    pub fn acquire(&mut self, scope: TrafficScope, bytes: u64, now: Instant) -> Acquire {
        let keys: [Option<LimitKey>; 2] = match scope {
            TrafficScope::Team { tenant_id, user_id } => [
                Some(LimitKey::Team(tenant_id)),
                Some(LimitKey::Member { tenant_id, user_id }),
            ],
            TrafficScope::Personal { tenant_id, .. } => [Some(LimitKey::Personal(tenant_id)), None],
            TrafficScope::Guest { endpoint_key } => [Some(LimitKey::Guest(endpoint_key)), None],
        };

        let mut wait = Duration::ZERO;
        for key in keys.into_iter().flatten() {
            let Some(bucket) = self.buckets.get_mut(&key) else {
                return Acquire::Missing(key);
            };
            bucket.refill(now);
            if bytes > bucket.rate.capacity() {
                return Acquire::Oversized {
                    requested: bytes,
                    capacity: bucket.rate.capacity(),
                };
            }
            wait = wait.max(bucket.wait_for(bytes));
        }

        if !wait.is_zero() {
            return Acquire::Wait(wait);
        }

        for key in keys.into_iter().flatten() {
            self.buckets
                .get_mut(&key)
                .expect("all buckets were checked")
                .available -= bytes;
        }
        Acquire::Ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BURST: Duration = Duration::from_millis(100);

    fn rate(bytes_per_second: u64) -> Rate {
        Rate::new(bytes_per_second, BURST).unwrap()
    }

    fn tenant(value: u128) -> TenantId {
        TenantId::from_u128(value)
    }

    fn user(value: u128) -> UserId {
        UserId::from_u128(value)
    }

    #[test]
    fn team_and_member_budgets_are_consumed_atomically() {
        let now = Instant::now();
        let mut limiter = AggregateLimiter::default();
        limiter.set_rate(LimitKey::Team(tenant(1)), rate(1_000), now);
        limiter.set_rate(
            LimitKey::Member {
                tenant_id: tenant(1),
                user_id: user(10),
            },
            rate(500),
            now,
        );
        let scope = TrafficScope::Team {
            tenant_id: tenant(1),
            user_id: user(10),
        };

        assert_eq!(limiter.acquire(scope, 50, now), Acquire::Ready);
        assert_eq!(limiter.acquire(scope, 1, now), Acquire::Wait(BURST / 50));

        let other_member = TrafficScope::Team {
            tenant_id: tenant(1),
            user_id: user(11),
        };
        limiter.set_rate(
            LimitKey::Member {
                tenant_id: tenant(1),
                user_id: user(11),
            },
            rate(500),
            now,
        );
        assert_eq!(limiter.acquire(other_member, 50, now), Acquire::Ready);
    }

    #[test]
    fn waiting_on_member_does_not_consume_team_budget() {
        let now = Instant::now();
        let mut limiter = AggregateLimiter::default();
        limiter.set_rate(LimitKey::Team(tenant(1)), rate(1_000), now);
        for user_id in [user(10), user(11)] {
            limiter.set_rate(
                LimitKey::Member {
                    tenant_id: tenant(1),
                    user_id,
                },
                rate(500),
                now,
            );
        }
        let first = TrafficScope::Team {
            tenant_id: tenant(1),
            user_id: user(10),
        };
        let second = TrafficScope::Team {
            tenant_id: tenant(1),
            user_id: user(11),
        };
        assert_eq!(limiter.acquire(first, 50, now), Acquire::Ready);
        assert!(matches!(limiter.acquire(first, 1, now), Acquire::Wait(_)));
        assert_eq!(limiter.acquire(second, 50, now), Acquire::Ready);
    }

    #[test]
    fn reconnects_share_the_same_personal_bucket() {
        let now = Instant::now();
        let mut limiter = AggregateLimiter::default();
        limiter.set_rate(LimitKey::Personal(tenant(7)), rate(500), now);
        let scope = TrafficScope::Personal {
            tenant_id: tenant(7),
            user_id: user(7),
        };
        assert_eq!(limiter.acquire(scope, 50, now), Acquire::Ready);
        assert!(matches!(limiter.acquire(scope, 1, now), Acquire::Wait(_)));
    }

    #[test]
    fn rate_reduction_does_not_refill_the_bucket() {
        let now = Instant::now();
        let mut limiter = AggregateLimiter::default();
        let key = LimitKey::Personal(tenant(7));
        let scope = TrafficScope::Personal {
            tenant_id: tenant(7),
            user_id: user(7),
        };
        limiter.set_rate(key, rate(1_000), now);
        assert_eq!(limiter.acquire(scope, 80, now), Acquire::Ready);
        limiter.set_rate(key, rate(500), now);
        assert!(matches!(limiter.acquire(scope, 21, now), Acquire::Wait(_)));
    }
}
