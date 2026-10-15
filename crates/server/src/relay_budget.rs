//! Integer allocation and immutable-lease reservation. No per-packet database work.
use std::collections::BTreeMap;

#[derive(Debug)]
pub(crate) struct BudgetClock {
    last: Option<(i64, std::time::Instant)>,
    blocked_until: std::time::Instant,
}
impl BudgetClock {
    pub fn new(now: std::time::Instant) -> Self {
        // At startup, old processes may still hold valid leases, even if the
        // database wall clock changed while this coordinator was down.
        Self {
            last: None,
            blocked_until: now + std::time::Duration::from_secs(15),
        }
    }
    pub fn observe(&mut self, database_ms: i64, now: std::time::Instant) -> bool {
        if let Some((previous, instant)) = self.last {
            let elapsed = now.saturating_duration_since(instant).as_millis() as i128;
            if (i128::from(database_ms) - i128::from(previous) - elapsed).abs() > 1000 {
                self.blocked_until = now + std::time::Duration::from_secs(15);
            }
        }
        self.last = Some((database_ms, now));
        now >= self.blocked_until
    }
}

/// Deterministic weighted shares whose sum is EXACTLY the total, including small totals.
pub(crate) fn weighted_shares(
    total: u64,
    weights: &BTreeMap<String, u32>,
) -> BTreeMap<String, u64> {
    let sum: u128 = weights.values().map(|v| u128::from(*v)).sum();
    if sum == 0 {
        return BTreeMap::new();
    }
    let mut shares: BTreeMap<_, _> = weights
        .iter()
        .map(|(id, weight)| {
            (
                id.clone(),
                ((u128::from(total) * u128::from(*weight)) / sum) as u64,
            )
        })
        .collect();
    let remainder = total - shares.values().sum::<u64>();
    // Floor rounding leaves less than one byte/sec per participating node.
    for value in shares.values_mut().take(remainder as usize) {
        *value += 1;
    }
    shares
}

pub(crate) const MIN_PACKET_CREDIT: u64 = 4096;

/// Keep unexpired participants and fill free packet-credit slots in FIFO order.
/// Dense accounts use short leases, avoiding permanent starvation of a small share.
pub(crate) fn packet_safe_shares(
    total: u64,
    participants: &[(String, u32, bool, i128)],
) -> (BTreeMap<String, u64>, BTreeMap<String, u64>, i64) {
    let burst = total / 10;
    let base = MIN_PACKET_CREDIT.min(burst).max(1);
    let slots = (burst / base).max(1) as usize;
    let mut order = participants.to_vec();
    order.sort_by(|a, b| b.2.cmp(&a.2).then(a.3.cmp(&b.3)).then(a.0.cmp(&b.0)));
    let weights: BTreeMap<_, _> = order
        .iter()
        .take(slots)
        .map(|(id, weight, _, _)| (id.clone(), *weight))
        .collect();
    let rates = weighted_shares(total, &weights);
    let mut credits = weighted_shares(
        burst.saturating_sub(base.saturating_mul(weights.len() as u64)),
        &weights,
    );
    for value in credits.values_mut() {
        *value += base;
    }
    (
        rates,
        credits,
        if participants.len() > slots {
            1000
        } else {
            15_000
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn small_shares_have_packet_credit_and_waiting_nodes_are_not_starved() {
        let mut participants: Vec<_> = (0..64)
            .map(|i| {
                (
                    format!("node-{i:02}"),
                    if i == 0 { 1000 } else { 1 },
                    false,
                    i as i128,
                )
            })
            .collect();
        let (rates, credits, ttl) = packet_safe_shares(625_000, &participants);
        assert_eq!(ttl, 1000);
        assert_eq!(rates.values().sum::<u64>(), 625_000);
        assert_eq!(credits.values().sum::<u64>(), 62_500);
        assert!(credits.values().all(|v| *v >= MIN_PACKET_CREDIT));
        for participant in &mut participants {
            if rates.contains_key(&participant.0) {
                participant.3 += 1000;
            }
        }
        let (next, _, _) = packet_safe_shares(625_000, &participants);
        assert!(next.keys().all(|node| !rates.contains_key(node)));
    }
    #[test]
    fn restart_and_database_clock_jumps_wait_out_previous_lease_lifetimes() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let mut clock = BudgetClock::new(start);
        assert!(!clock.observe(1000, start));
        assert!(clock.observe(16000, start + Duration::from_secs(15)));
        assert!(!clock.observe(116000, start + Duration::from_secs(16)));
        assert!(!clock.observe(130000, start + Duration::from_secs(30)));
        assert!(clock.observe(131000, start + Duration::from_secs(31)));
        assert!(!clock.observe(1000, start + Duration::from_secs(32)));
    }
    #[test]
    fn integer_shares_never_multiply_budget_and_single_node_gets_full_rate() {
        for nodes in 1..=64 {
            let weights = (0..nodes)
                .map(|i| (format!("n{i:02}"), (i + 1) as u32))
                .collect();
            for total in [0, 1, 2, 7, 100, 125_000, 1_250_000, u64::MAX] {
                let shares = weighted_shares(total, &weights);
                assert_eq!(
                    shares.values().map(|v| u128::from(*v)).sum::<u128>(),
                    u128::from(total)
                );
                if nodes == 1 {
                    assert_eq!(shares["n00"], total);
                }
            }
        }
    }
}
