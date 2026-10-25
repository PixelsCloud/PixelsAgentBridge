use crate::{DesktopCaptureTimings, RequestId};
use serde::{Deserialize, Serialize};

pub const DESKTOP_TRACE_VERSION: u16 = 1;
pub const DESKTOP_TRACE_FRAMES: usize = 64;
pub const DESKTOP_TRACE_SLOW_FRAMES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopCaptureBreakdown {
    pub native_total_us: u64,
    pub readback_us: u64,
    pub conversion_us: u64,
    pub codec_us: u64,
    pub worker_roundtrip_us: u64,
    pub helper_roundtrip_us: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maximum_bounded_trace_fits_the_control_frame_and_rejects_bad_history() {
        let frame = DesktopFrameTrace { frame_id:1, elapsed_us:u64::MAX, capture_time_us:u64::MAX,
            credit_us:u64::MAX, budget_us:u64::MAX, source_us:u64::MAX,
            capture:Some(DesktopCaptureTimings { wait_us:u64::MAX, encode_us:u64::MAX,
                detail:Some(DesktopCaptureBreakdown { native_total_us:u64::MAX, readback_us:u64::MAX,
                    conversion_us:u64::MAX, codec_us:u64::MAX, worker_roundtrip_us:u64::MAX, helper_roundtrip_us:u64::MAX }) }),
            send_us:Some(u64::MAX), ack_us:Some(u64::MAX) };
        let mut snapshot = DesktopTraceSnapshot { version:DESKTOP_TRACE_VERSION, session_id:RequestId::new(),
            generation:u64::MAX, latest_frame_id:u64::MAX,
            frames:(0..DESKTOP_TRACE_FRAMES).map(|i| DesktopFrameTrace { frame_id:u64::MAX-DESKTOP_TRACE_FRAMES as u64+i as u64+1, ..frame.clone() }).collect(),
            slow_frames:vec![frame; DESKTOP_TRACE_SLOW_FRAMES], network:None };
        snapshot.validate().unwrap();
        let bytes = serde_json::to_vec(&snapshot).unwrap();
        assert!(bytes.len()+1024 < crate::DESKTOP_CONTROL_BYTES, "{}", bytes.len());
        assert_eq!(serde_json::from_slice::<DesktopTraceSnapshot>(&bytes).unwrap(), snapshot);
        snapshot.frames.push(snapshot.frames[0].clone());
        assert!(snapshot.validate().is_err());
        snapshot.frames.pop(); snapshot.frames.swap(0,1);
        assert!(snapshot.validate().is_err());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopFrameTrace {
    pub frame_id: u64,
    /// Producer clock, relative to this generation; never subtract peer clocks.
    pub elapsed_us: u64,
    pub capture_time_us: u64,
    pub credit_us: u64,
    pub budget_us: u64,
    pub source_us: u64,
    pub capture: Option<DesktopCaptureTimings>,
    /// Submission to QUIC, not delivery to the viewer.
    pub send_us: Option<u64>,
    /// Send start to returned display ACK, using the producer clock.
    pub ack_us: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopNetworkTrace {
    pub path_id: String,
    pub rtt_us: u64,
    pub lost_packets: u64,
    pub lost_bytes: u64,
    pub tx_datagrams: u64,
    pub rx_datagrams: u64,
    pub congestion_events: u64,
    pub cwnd: u64,
    pub stream_frames_tx: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopTraceSnapshot {
    pub version: u16,
    pub session_id: RequestId,
    pub generation: u64,
    pub latest_frame_id: u64,
    pub frames: Vec<DesktopFrameTrace>,
    pub slow_frames: Vec<DesktopFrameTrace>,
    pub network: Option<DesktopNetworkTrace>,
}
impl DesktopTraceSnapshot {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != DESKTOP_TRACE_VERSION || self.session_id.as_uuid().is_nil()
            || self.generation == 0 || self.frames.len() > DESKTOP_TRACE_FRAMES
            || self.slow_frames.len() > DESKTOP_TRACE_SLOW_FRAMES
            || self.frames.windows(2).any(|w| w[0].frame_id >= w[1].frame_id)
            || self.frames.iter().chain(&self.slow_frames)
                .any(|f| f.frame_id == 0 || f.frame_id > self.latest_frame_id)
            || self.network.as_ref().is_some_and(|n| n.path_id.len() > 64)
        {
            return Err("invalid desktop frame trace");
        }
        Ok(())
    }
}
