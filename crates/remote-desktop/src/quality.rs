//! Encoder budgets use sender-clock credit pressure; never subtract peer clocks.
use std::time::Duration;
use tokio::time::Instant;

const BUDGETS: [u32; 6] = [0, 3_000_000, 1_500_000, 750_000, 350_000, 150_000];
const DOWN_INTERVAL: Duration = Duration::from_secs(2);
const UP_INTERVAL: Duration = Duration::from_secs(10);

pub(crate) struct EncodingQuality {
    level: usize,
    changed: Option<Instant>,
    pressure: Option<Instant>,
}
impl EncodingQuality {
    pub(crate) fn new() -> Self {
        Self { level: 0, changed: None, pressure: None }
    }
    pub(crate) fn pressure(&mut self, now: Instant) {
        self.pressure = Some(now);
        if self.changed.is_none_or(|last| now.saturating_duration_since(last) >= DOWN_INTERVAL)
            && self.level + 1 < BUDGETS.len() {
            self.level += 1;
            self.changed = Some(now);
        }
    }
    pub(crate) fn consumed(&mut self, now: Instant) {
        if self.level != 0
            && self.pressure.is_some_and(|last| now.saturating_duration_since(last) >= UP_INTERVAL)
            && self.changed.is_some_and(|last| now.saturating_duration_since(last) >= UP_INTERVAL) {
            self.level -= 1;
            self.changed = Some(now);
        }
    }
    /// Zero restores the backend's original full-quality settings.
    pub(crate) fn budget(&self) -> u32 { BUDGETS[self.level] }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sustained_pressure_is_bounded_and_recovery_requires_stability() {
        let start = Instant::now();
        let mut q = EncodingQuality::new();
        assert_eq!(q.budget(), 0);
        q.pressure(start);
        assert_eq!(q.budget(), 3_000_000);
        for n in 1..20 { q.pressure(start + Duration::from_millis(n * 50)); }
        assert_eq!(q.budget(), 3_000_000);
        for n in 1..20 { q.pressure(start + Duration::from_secs(n * 2)); }
        assert_eq!(q.budget(), 150_000);
        q.consumed(start + Duration::from_secs(47));
        assert_eq!(q.budget(), 150_000);
        for n in 1..=5 { q.consumed(start + Duration::from_secs(38 + n * 10)); }
        assert_eq!(q.budget(), 0);
    }
}
