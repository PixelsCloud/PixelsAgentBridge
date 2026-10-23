//! Viewer-clock measurements. A quiet desktop with no outstanding frames is
//! idle, not a decoder stall. Only metadata is retained, bounded to four frames.
use serde::Serialize;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize)]
pub struct ViewerDiagnosticsSnapshot {
    pub received_frame_id: u64,
    pub submitted_frame_id: u64,
    pub presented_frame_id: u64,
    pub received_bytes: u64,
    pub receive_fps_milli: u64,
    pub present_fps_milli: u64,
    pub bitrate_bps: u64,
    pub last_received_ms: Option<u64>,
    pub last_presented_ms: Option<u64>,
    pub pending_frames: usize,
    pub oldest_pending_ms: Option<u64>,
    pub submit_last_us: Option<u64>,
    pub receive_to_present_last_us: Option<u64>,
    pub stage: &'static str,
}
pub struct ViewerDiagnostics {
    received: u64,
    submitted: u64,
    presented: u64,
    bytes: u64,
    last_received: Option<Instant>,
    last_presented: Option<Instant>,
    pending: VecDeque<(u64, Instant)>,
    submitting: Option<u64>,
    submit_us: Option<u64>,
    presentation_us: Option<u64>,
    sampled: Instant,
    baseline: (u64, u64, u64),
}
fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}
fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}
impl ViewerDiagnostics {
    pub fn new(now: Instant) -> Self {
        Self {
            received: 0,
            submitted: 0,
            presented: 0,
            bytes: 0,
            last_received: None,
            last_presented: None,
            pending: VecDeque::new(),
            submitting: None,
            submit_us: None,
            presentation_us: None,
            sampled: now,
            baseline: (0, 0, 0),
        }
    }
    pub fn received(&mut self, id: u64, bytes: usize, now: Instant) {
        self.received = id;
        self.bytes = self.bytes.saturating_add(bytes as u64);
        self.last_received = Some(now);
        self.submitting = Some(id);
        if self.pending.len() == pab_protocol::DESKTOP_MAX_IN_FLIGHT as usize {
            self.pending.pop_front();
        }
        self.pending.push_back((id, now));
    }
    pub fn submitted(&mut self, id: u64, elapsed: Duration) {
        self.submitted = id;
        self.submit_us = Some(micros(elapsed));
        if self.submitting == Some(id) {
            self.submitting = None;
        }
    }
    pub fn presented(&mut self, id: u64, now: Instant) {
        self.presented = id;
        self.last_presented = Some(now);
        // Native presentation notifications may coalesce. Measure the newest
        // actually presented frame, not each skipped watch notification.
        while self.pending.front().is_some_and(|(frame, _)| *frame <= id) {
            let (frame, at) = self.pending.pop_front().unwrap();
            if frame == id {
                self.presentation_us = Some(micros(now.saturating_duration_since(at)));
            }
        }
    }
    pub fn snapshot(&mut self, now: Instant) -> ViewerDiagnosticsSnapshot {
        let elapsed = now
            .saturating_duration_since(self.sampled)
            .as_micros()
            .max(1);
        let rate = |delta: u64, scale: u128| {
            ((delta as u128 * scale) / elapsed).min(u64::MAX as u128) as u64
        };
        let snapshot = ViewerDiagnosticsSnapshot {
            received_frame_id: self.received,
            submitted_frame_id: self.submitted,
            presented_frame_id: self.presented,
            received_bytes: self.bytes,
            receive_fps_milli: rate(self.received.saturating_sub(self.baseline.0), 1_000_000_000),
            present_fps_milli: rate(
                self.presented.saturating_sub(self.baseline.1),
                1_000_000_000,
            ),
            bitrate_bps: rate(self.bytes.saturating_sub(self.baseline.2), 8_000_000),
            last_received_ms: self
                .last_received
                .map(|at| millis(now.saturating_duration_since(at))),
            last_presented_ms: self
                .last_presented
                .map(|at| millis(now.saturating_duration_since(at))),
            pending_frames: self.pending.len(),
            oldest_pending_ms: self
                .pending
                .front()
                .map(|(_, at)| millis(now.saturating_duration_since(*at))),
            submit_last_us: self.submit_us,
            receive_to_present_last_us: self.presentation_us,
            stage: if self.submitting.is_some() {
                "submitting"
            } else if !self.pending.is_empty() {
                "presentation"
            } else if self.received == 0 {
                "first_frame"
            } else {
                "idle"
            },
        };
        self.sampled = now;
        self.baseline = (self.received, self.presented, self.bytes);
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_and_pending_are_distinct_and_rates_use_elapsed_time() {
        let start = Instant::now();
        let mut counters = ViewerDiagnostics::new(start);
        counters.received(1, 1000, start);
        let s = counters.snapshot(start + Duration::from_millis(10));
        assert_eq!(s.stage, "submitting");
        assert_eq!(s.oldest_pending_ms, Some(10));
        counters.submitted(1, Duration::from_millis(12));
        assert_eq!(
            counters.snapshot(start + Duration::from_millis(15)).stage,
            "presentation"
        );
        counters.presented(1, start + Duration::from_millis(20));
        let s = counters.snapshot(start + Duration::from_millis(25));
        assert_eq!(s.receive_to_present_last_us, Some(20_000));
        assert_eq!(s.pending_frames, 0);
        let s = counters.snapshot(start + Duration::from_secs(60));
        assert_eq!(s.stage, "idle");
        assert_eq!(s.receive_fps_milli, 0);
        assert_eq!(s.bitrate_bps, 0);
        assert_eq!(s.oldest_pending_ms, None);
    }
    #[test]
    fn coalesced_presentation_uses_newest_frame_and_metadata_stays_bounded() {
        let start = Instant::now();
        let mut counters = ViewerDiagnostics::new(start);
        for id in 1..=4 {
            counters.received(id, 1000, start + Duration::from_millis(id));
            counters.submitted(id, Duration::ZERO);
        }
        counters.presented(4, start + Duration::from_millis(24));
        let s = counters.snapshot(start + Duration::from_secs(2));
        assert_eq!(s.presented_frame_id, 4);
        assert_eq!(s.receive_to_present_last_us, Some(20_000));
        assert_eq!(s.receive_fps_milli, 2000);
        assert_eq!(s.present_fps_milli, 2000);
        assert_eq!(s.bitrate_bps, 16_000);
        assert!(counters.pending.is_empty());
        for id in 5..=50 {
            counters.received(id, 1, start);
        }
        assert_eq!(counters.pending.len(), 4);
    }
}
