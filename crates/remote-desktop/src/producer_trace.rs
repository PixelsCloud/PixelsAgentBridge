use pab_protocol::*;
use std::{collections::VecDeque, time::Duration};
use tokio::time::Instant;

fn micros(d: Duration) -> u64 { d.as_micros().min(u64::MAX as u128) as u64 }
struct Entry { frame: DesktopFrameTrace, sent_at: Instant }
pub(crate) struct FrameHistory {
    created: Instant,
    latest: u64,
    frames: VecDeque<Entry>,
    slow: VecDeque<DesktopFrameTrace>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_and_slow_retention_are_bounded_and_cumulative_ack_is_idempotent() {
        let mut history = FrameHistory::new();
        for frame_id in 1..=100 {
            history.begin(DesktopFrameTrace {
                frame_id, elapsed_us: 0, capture_time_us: frame_id,
                credit_us: 0, budget_us: 0, source_us: 120_000,
                capture: None, send_us: None, ack_us: None,
            });
            history.sent(frame_id, Duration::from_millis(1));
        }
        assert_eq!(history.latest, 100);
        assert_eq!(history.frames.len(), DESKTOP_TRACE_FRAMES);
        assert_eq!(history.frames.front().unwrap().frame.frame_id, 37);
        assert_eq!(history.slow.len(), DESKTOP_TRACE_SLOW_FRAMES);
        history.acknowledge(100);
        let first_ack = history.frames.back().unwrap().frame.ack_us;
        history.acknowledge(100);
        assert!(first_ack.is_some());
        assert_eq!(history.frames.back().unwrap().frame.ack_us, first_ack);
        assert!(history.frames.iter().all(|e| e.frame.ack_us.is_some()));
        assert_eq!(history.slow.len(), DESKTOP_TRACE_SLOW_FRAMES);
        assert_eq!(history.slow.front().unwrap().frame_id, 93);
        assert_eq!(history.slow.back().unwrap().frame_id, 100);
    }
}
impl FrameHistory {
    pub(crate) fn new() -> Self {
        Self { created: Instant::now(), latest: 0,
            frames: VecDeque::with_capacity(DESKTOP_TRACE_FRAMES),
            slow: VecDeque::with_capacity(DESKTOP_TRACE_SLOW_FRAMES) }
    }
    pub(crate) fn begin(&mut self, mut frame: DesktopFrameTrace) {
        let now = Instant::now();
        frame.elapsed_us = micros(now.saturating_duration_since(self.created));
        self.latest = frame.frame_id;
        if self.frames.len() == DESKTOP_TRACE_FRAMES { self.frames.pop_front(); }
        self.frames.push_back(Entry { frame, sent_at: now });
    }
    fn retain_slow(&mut self, frame: DesktopFrameTrace) {
        if frame.credit_us.max(frame.source_us).max(frame.send_us.unwrap_or(0))
            .max(frame.ack_us.unwrap_or(0)) < 100_000 { return; }
        if let Some(old) = self.slow.iter_mut().find(|f| f.frame_id == frame.frame_id) { *old = frame; return; }
        if self.slow.len() == DESKTOP_TRACE_SLOW_FRAMES { self.slow.pop_front(); }
        self.slow.push_back(frame);
    }
    pub(crate) fn sent(&mut self, id: u64, elapsed: Duration) {
        if let Some(entry) = self.frames.iter_mut().find(|e| e.frame.frame_id == id) {
            entry.frame.send_us = Some(micros(elapsed));
            let frame = entry.frame.clone();
            self.retain_slow(frame);
        }
    }
    pub(crate) fn acknowledge(&mut self, id: u64) {
        let now = Instant::now();
        let mut updated = Vec::with_capacity(DESKTOP_MAX_IN_FLIGHT as usize);
        for entry in &mut self.frames {
            if entry.frame.frame_id <= id && entry.frame.ack_us.is_none() {
                entry.frame.ack_us = Some(micros(now.saturating_duration_since(entry.sent_at)));
                updated.push(entry.frame.clone());
            }
        }
        for frame in updated { self.retain_slow(frame); }
    }
    pub(crate) fn snapshot(&self, config: &DesktopVideoConfig, after_frame_id: u64, network: Option<pab_transport::ConnectionStatistics>) -> DesktopTraceSnapshot {
        DesktopTraceSnapshot {
            version: DESKTOP_TRACE_VERSION, session_id: config.session_id, generation: config.generation,
            latest_frame_id: self.latest,
            frames: self.frames.iter().filter(|e| e.frame.frame_id >= after_frame_id.saturating_sub(DESKTOP_MAX_IN_FLIGHT-1))
                .map(|e| e.frame.clone()).collect(),
            slow_frames: self.slow.iter().cloned().collect(),
            network: network.map(|n| DesktopNetworkTrace {
                path_id:n.path_id, rtt_us:n.rtt_us, lost_packets:n.lost_packets, lost_bytes:n.lost_bytes,
                tx_datagrams:n.tx_datagrams, rx_datagrams:n.rx_datagrams, congestion_events:n.congestion_events,
                cwnd:n.cwnd, stream_frames_tx:n.stream_frames_tx,
            }),
        }
    }
}
