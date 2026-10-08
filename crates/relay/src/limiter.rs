use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use pab_protocol::{EndpointKey, TrafficScope, UserId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LimitKey {
    User(UserId),
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

    pub fn retain(&mut self, now: Instant, mut keep: impl FnMut(LimitKey) -> bool) {
        // Briefly retain removed scopes until they could naturally refill.
        // Logout/login must not manufacture a fresh burst of traffic credit.
        self.buckets.retain(|key, bucket| {
            keep(*key) || now.saturating_duration_since(bucket.updated_at) < bucket.rate.burst
        });
    }

    pub fn acquire(&mut self, scope: TrafficScope, bytes: u64, now: Instant) -> Acquire {
        let keys: [Option<LimitKey>; 2] = match scope {
            TrafficScope::User { user_id } => [Some(LimitKey::User(user_id)), None],
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
    fn rate(bytes: u64) -> Rate {
        Rate::new(bytes, Duration::from_millis(100)).unwrap()
    }

    #[test]
    fn connections_for_one_user_share_budget_but_other_users_do_not() {
        let now = Instant::now();
        let mut limiter = AggregateLimiter::default();
        let first = UserId::new();
        let second = UserId::new();
        for user in [first, second] {
            limiter.set_rate(LimitKey::User(user), rate(500), now);
        }
        assert_eq!(
            limiter.acquire(TrafficScope::User { user_id: first }, 50, now),
            Acquire::Ready
        );
        assert_eq!(
            limiter.acquire(TrafficScope::User { user_id: first }, 1, now),
            Acquire::Wait(Duration::from_millis(2))
        );
        assert_eq!(
            limiter.acquire(TrafficScope::User { user_id: second }, 50, now),
            Acquire::Ready
        );
    }

    #[test]
    fn refreshing_and_reducing_rate_do_not_refill_quota() {
        let now = Instant::now();
        let mut limiter = AggregateLimiter::default();
        let user_id = UserId::new();
        let key = LimitKey::User(user_id);
        let scope = TrafficScope::User { user_id };
        limiter.set_rate(key, rate(1_000), now);
        assert_eq!(limiter.acquire(scope, 80, now), Acquire::Ready);
        limiter.set_rate(key, rate(1_000), now);
        limiter.set_rate(key, rate(500), now);
        assert!(matches!(limiter.acquire(scope, 21, now), Acquire::Wait(_)));
        assert_eq!(
            limiter.acquire(scope, 21, now + Duration::from_millis(2)),
            Acquire::Ready
        );
    }

    #[test]
    fn guests_have_independent_endpoint_quotas_and_unknown_scopes_fail() {
        let now = Instant::now();
        let mut limiter = AggregateLimiter::default();
        let first = EndpointKey::new([1; 32]);
        let second = EndpointKey::new([2; 32]);
        limiter.set_rate(LimitKey::Guest(first), rate(500), now);
        limiter.set_rate(LimitKey::Guest(second), rate(500), now);
        assert_eq!(
            limiter.acquire(
                TrafficScope::Guest {
                    endpoint_key: first
                },
                50,
                now
            ),
            Acquire::Ready
        );
        assert!(matches!(
            limiter.acquire(
                TrafficScope::Guest {
                    endpoint_key: first
                },
                1,
                now
            ),
            Acquire::Wait(_)
        ));
        assert_eq!(
            limiter.acquire(
                TrafficScope::Guest {
                    endpoint_key: second
                },
                50,
                now
            ),
            Acquire::Ready
        );
        assert!(matches!(
            limiter.acquire(
                TrafficScope::User {
                    user_id: UserId::new()
                },
                1,
                now
            ),
            Acquire::Missing(_)
        ));
        assert!(matches!(
            limiter.acquire(
                TrafficScope::Guest {
                    endpoint_key: second
                },
                51,
                now
            ),
            Acquire::Oversized { .. }
        ));
    }
}
