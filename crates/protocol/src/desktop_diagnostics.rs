use crate::RequestId;
use serde::{Deserialize, Serialize};

pub const DESKTOP_DIAGNOSTICS_VERSION: u16 = 1;

/// All durations use a single endpoint's monotonic clock. Capture timestamps
/// in video headers are presentation timestamps, never wall-clock latency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopCaptureTimings {
    /// Waiting for a changed image, including capture scheduling; static
    /// desktops can legitimately spend an arbitrarily long time here.
    pub wait_us: u64,
    /// The native encode call, including conversion/readback if required.
    pub encode_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopProducerStage {
    Starting,
    Credit,
    Capture,
    Send,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopDiagnosticsSnapshot {
    pub version: u16,
    pub session_id: RequestId,
    pub generation: u64,
    pub stage: DesktopProducerStage,
    pub stage_age_ms: u64,
    pub produced_frames: u64,
    pub sent_frame_id: u64,
    pub presented_frame_id: u64,
    pub encoded_bytes: u64,
    pub capture: Option<DesktopCaptureTimings>,
    /// Source request including helper IPC, waiting for changes and encoding.
    pub source_last_us: Option<u64>,
    /// Writing compressed data to QUIC, not delivery or one-way network time.
    pub send_last_us: Option<u64>,
    /// Send start to returned presentation ACK: includes transport and viewer.
    pub presentation_ack_last_us: Option<u64>,
    pub oldest_unpresented_ms: Option<u64>,
}
impl DesktopDiagnosticsSnapshot {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != DESKTOP_DIAGNOSTICS_VERSION
            || self.session_id.as_uuid().is_nil()
            || self.generation == 0
            || self.presented_frame_id > self.sent_frame_id
            || self.sent_frame_id > self.produced_frames
            || self.sent_frame_id - self.presented_frame_id > super::DESKTOP_MAX_IN_FLIGHT
            || self.oldest_unpresented_ms.is_some()
                != (self.sent_frame_id > self.presented_frame_id)
        {
            return Err("invalid desktop diagnostics snapshot");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_impossible_progress_without_changing_legacy_quality_wire() {
        let mut snapshot = DesktopDiagnosticsSnapshot {
            version: DESKTOP_DIAGNOSTICS_VERSION,
            session_id: RequestId::new(),
            generation: 1,
            stage: DesktopProducerStage::Capture,
            stage_age_ms: 60000,
            produced_frames: 4,
            sent_frame_id: 4,
            presented_frame_id: 0,
            encoded_bytes: 123,
            capture: None,
            source_last_us: None,
            send_last_us: None,
            presentation_ack_last_us: None,
            oldest_unpresented_ms: Some(10),
        };
        snapshot.validate().unwrap();
        snapshot.sent_frame_id = 5;
        assert!(snapshot.validate().is_err());
        snapshot.produced_frames = 5;
        assert!(snapshot.validate().is_err());
        snapshot.presented_frame_id = 5;
        assert!(snapshot.validate().is_err());
        snapshot.oldest_unpresented_ms = None;
        snapshot.validate().unwrap();
        snapshot.presented_frame_id = 6;
        assert!(snapshot.validate().is_err());
        let legacy: super::super::DesktopQualitySnapshot =
            serde_json::from_value(serde_json::json!({
                "version":1,"session_id":RequestId::new(),"generation":1,
                "capture_fps_milli":15000,"encoder_target_bps":null
            }))
            .unwrap();
        legacy.validate().unwrap();
        assert_eq!(
            serde_json::to_value(legacy)
                .unwrap()
                .as_object()
                .unwrap()
                .len(),
            5
        );
    }
}
