//! Union of productive work intervals, independent of wall-clock timestamps.
use std::{sync::{Arc, Mutex}, time::{Duration, Instant}};

#[derive(Default)]
struct State { workers: usize, enabled: bool, since: Option<Instant>, elapsed: Duration }
impl State {
    fn settle(&mut self, now: Instant) {
        if let Some(since) = self.since.take() { self.elapsed = self.elapsed.saturating_add(now.saturating_duration_since(since)); }
    }
    fn rearm(&mut self, now: Instant) { self.since = (self.enabled && self.workers > 0).then_some(now); }
}

#[derive(Default)]
pub(super) struct ActiveBatchClock(Mutex<State>);
pub(super) struct Activity(Arc<ActiveBatchClock>);
impl ActiveBatchClock {
    pub fn reset(&self, enabled: bool) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = State {enabled, ..State::default()};
    }
    pub fn set_enabled(&self, enabled: bool) { self.set_enabled_at(enabled, Instant::now()); }
    fn set_enabled_at(&self, enabled: bool, now: Instant) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.settle(now); state.enabled = enabled; state.rearm(now);
    }
    pub fn enter(self: &Arc<Self>) -> Activity {
        self.enter_at(Instant::now()); Activity(self.clone())
    }
    fn enter_at(&self, now: Instant) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.settle(now); state.workers += 1; state.rearm(now);
    }
    fn leave_at(&self, now: Instant) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.settle(now); state.workers = state.workers.saturating_sub(1); state.rearm(now);
    }
    pub fn milliseconds(&self) -> u64 { self.milliseconds_at(Instant::now()) }
    fn milliseconds_at(&self, now: Instant) -> u64 {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.settle(now); state.rearm(now); state.elapsed.as_millis().min(u64::MAX as u128) as u64
    }
}
impl Drop for Activity { fn drop(&mut self) { self.0.leave_at(Instant::now()); } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parallel_work_is_a_union_and_pause_idle_and_restart_are_excluded() {
        let clock=ActiveBatchClock::default(); clock.reset(true);
        let base=Instant::now(); let at=|ms|base+Duration::from_millis(ms);
        clock.enter_at(at(1_000)); clock.enter_at(at(2_000));
        assert_eq!(clock.milliseconds_at(at(3_000)),2_000);
        clock.set_enabled_at(false,at(3_500));
        assert_eq!(clock.milliseconds_at(at(30_000)),2_500);
        clock.leave_at(at(31_000)); clock.leave_at(at(32_000));
        clock.set_enabled_at(true,at(40_000));
        assert_eq!(clock.milliseconds_at(at(60_000)),2_500);
        clock.enter_at(at(61_000)); clock.leave_at(at(62_500));
        assert_eq!(clock.milliseconds_at(at(100_000)),4_000);
        clock.reset(true); assert_eq!(clock.milliseconds_at(at(200_000)),0);
    }
    #[test]
    fn cancellation_drops_activity_and_rounding_preserves_submillisecond_segments() {
        let clock=Arc::new(ActiveBatchClock::default()); clock.reset(true);
        { let _activity=clock.enter(); }
        let recorded=clock.milliseconds(); assert_eq!(clock.milliseconds(),recorded);
        let base=Instant::now();
        for index in 0..10 { let start=base+Duration::from_micros(index*200);clock.enter_at(start);clock.leave_at(start+Duration::from_micros(150)); }
        assert!(clock.milliseconds_at(base+Duration::from_secs(1))>=1);
    }
}
