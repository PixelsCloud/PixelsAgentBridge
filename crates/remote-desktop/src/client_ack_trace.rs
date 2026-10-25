//! Bounded metadata only. Observation never grants video/control credit.
use std::{collections::VecDeque, time::Duration};
use tokio::time::Instant;

#[derive(Debug, Clone, serde::Serialize)]
pub struct DesktopAckTrace {
    pub generation: u64,
    pub frame_id: u64,
    pub queue_us: Option<u64>,
    pub write_us: Option<u64>,
    pub reply_us: Option<u64>,
    pub pending_before: Option<usize>,
}
struct Entry {
    frame: DesktopAckTrace,
    notified: Instant,
}
#[derive(Default)]
pub(super) struct AckHistory(VecDeque<Entry>);
fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}
impl AckHistory {
    pub(super) fn notified(&mut self, generation: u64, frame_id: u64) {
        if self.0.len() == 64 { self.0.pop_front(); }
        self.0.push_back(Entry {
            frame: DesktopAckTrace { generation, frame_id, queue_us: None,
                write_us: None, reply_us: None, pending_before: None },
            notified: Instant::now(),
        });
    }
    pub(super) fn sending(&mut self, id: (u64, u64), pending: usize) {
        if let Some(entry) = self.0.iter_mut().find(|e| (e.frame.generation, e.frame.frame_id) == id) {
            entry.frame.queue_us = Some(micros(entry.notified.elapsed()));
            entry.frame.pending_before = Some(pending);
        }
    }
    pub(super) fn written(&mut self, id: (u64, u64), elapsed: Duration) {
        if let Some(entry) = self.0.iter_mut().find(|e| (e.frame.generation, e.frame.frame_id) == id) {
            entry.frame.write_us = Some(micros(elapsed));
        }
    }
    pub(super) fn replied(&mut self, id: (u64, u64), elapsed: Duration) {
        if let Some(entry) = self.0.iter_mut().find(|e| (e.frame.generation, e.frame.frame_id) == id) {
            entry.frame.reply_us = Some(micros(elapsed));
        }
    }
    pub(super) fn snapshot(&self, generation: u64) -> Vec<DesktopAckTrace> {
        self.0.iter().filter(|e| e.frame.generation == generation).map(|e| e.frame.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_is_bounded_and_evicted_or_other_generation_frames_are_not_recreated() {
        let mut history = AckHistory::default();
        for frame in 1..=128 {
            history.notified(if frame <= 64 { 1 } else { 2 }, frame);
        }
        assert!(history.snapshot(1).is_empty());
        assert_eq!(history.snapshot(2).len(), 64);
        history.sending((1, 1), 8);
        assert!(history.snapshot(1).is_empty());
        history.sending((2, 128), 3);
        history.written((2, 128), Duration::from_micros(7));
        history.replied((2, 128), Duration::from_millis(50));
        let latest = history.snapshot(2).pop().unwrap();
        assert!(latest.queue_us.is_some());
        assert_eq!(latest.write_us, Some(7));
        assert_eq!(latest.reply_us, Some(50_000));
        assert_eq!(latest.pending_before, Some(3));
        assert_eq!(history.snapshot(2).len(), 64);
    }
}
