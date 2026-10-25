use pab_protocol::*;
use std::time::Duration;
use tokio::time::Instant;
#[path = "producer_trace.rs"]
mod trace;

pub(crate) struct ProducerDiagnostics {
    pub(super) trace: trace::FrameHistory,
    stage: DesktopProducerStage,
    since: Instant,
    produced: u64,
    bytes: u64,
    capture: Option<DesktopCaptureTimings>,
    source_us: Option<u64>,
    send_us: Option<u64>,
}
impl ProducerDiagnostics {
    pub(crate) fn new() -> Self {
        Self {
            trace: trace::FrameHistory::new(),
            stage: DesktopProducerStage::Starting,
            since: Instant::now(),
            produced: 0,
            bytes: 0,
            capture: None,
            source_us: None,
            send_us: None,
        }
    }
    pub(crate) fn stage(&mut self, stage: DesktopProducerStage) {
        self.stage = stage;
        self.since = Instant::now();
    }
    pub(crate) fn produced(
        &mut self,
        elapsed: Duration,
        bytes: usize,
        capture: Option<DesktopCaptureTimings>,
    ) {
        self.produced = self.produced.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes as u64);
        self.source_us = Some(elapsed.as_micros().min(u64::MAX as u128) as u64);
        self.capture = capture;
    }
    pub(crate) fn sent(&mut self, elapsed: Duration) {
        self.send_us = Some(elapsed.as_micros().min(u64::MAX as u128) as u64);
    }
    pub(crate) fn snapshot(
        &self,
        config: &DesktopVideoConfig,
        flow: &crate::flow::VideoFlow,
        now: Instant,
    ) -> DesktopDiagnosticsSnapshot {
        let (sent, presented, ack, oldest) = flow.diagnostics(now);
        DesktopDiagnosticsSnapshot {
            version: DESKTOP_DIAGNOSTICS_VERSION,
            session_id: config.session_id,
            generation: config.generation,
            stage: self.stage,
            stage_age_ms: now
                .saturating_duration_since(self.since)
                .as_millis()
                .min(u64::MAX as u128) as u64,
            produced_frames: self.produced,
            sent_frame_id: sent,
            presented_frame_id: presented,
            encoded_bytes: self.bytes,
            capture: self.capture.clone().map(|mut c| { c.detail = None; c }),
            source_last_us: self.source_us,
            send_last_us: self.send_us,
            presentation_ack_last_us: ack,
            oldest_unpresented_ms: oldest,
        }
    }
}
