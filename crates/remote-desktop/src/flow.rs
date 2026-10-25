//! Display credit is the rate limiter for a slow consumer. Keep the requested
//! cadence when credit returns, without turning a past network pause into a
//! second, persistent frame-rate penalty. Encoder budgets still adapt.
use crate::{DesktopError, Result};
use pab_protocol::{DESKTOP_MAX_IN_FLIGHT, DesktopErrorCode};
use std::{collections::VecDeque, time::Duration};
use tokio::time::Instant;

pub(crate) const MAX_BACKLOG_AGE: Duration = Duration::from_secs(5);

struct Flight {
    id: u64,
    started: Instant,
}

pub(crate) struct VideoFlow {
    sent: u64,
    acknowledged: u64,
    flights: VecDeque<Flight>,
    period: Duration,
    credit_wait_since: Option<Instant>,
    last_capture: Option<Instant>,
    quality: crate::quality::EncodingQuality,
    presentation_ack_last_us: Option<u64>,
}

impl VideoFlow {
    pub(crate) fn new(fps: u16) -> Self {
        let period = Duration::from_secs_f64(1.0 / f64::from(fps));
        Self {
            sent: 0,
            acknowledged: 0,
            flights: VecDeque::with_capacity(DESKTOP_MAX_IN_FLIGHT as usize),
            period,
            credit_wait_since: None,
            last_capture: None,
            quality: crate::quality::EncodingQuality::new(),
            presentation_ack_last_us: None,
        }
    }

    /// A screen/codec generation retires its frames and credit history, but the
    /// same viewer still consumes the same connection. Apply its established
    /// budget before the replacement encoder produces its first keyframe.
    pub(crate) fn restart(&self, fps: u16) -> Self {
        let mut fresh = Self::new(fps);
        fresh.quality = self.quality.clone();
        fresh
    }

    /// None means no credit. A timestamp in the past is immediately eligible;
    /// after a long static interval there is no accumulated burst allowance.
    pub(crate) fn capture_at(&mut self, now: Instant) -> Option<Instant> {
        if self.flights.len() == DESKTOP_MAX_IN_FLIGHT as usize {
            // Time before the next scheduled capture is not backpressure.
            self.credit_wait_since.get_or_insert(
                self.last_capture
                    .map_or(now, |last| (last + self.period).max(now)),
            );
            return None;
        }
        if let Some(since) = self.credit_wait_since.take() {
            let waited = now.saturating_duration_since(since);
            if waited > self.period.max(Duration::from_millis(100)) {
                self.quality.pressure(now);
                // The full flight window already stopped production during
                // this wait. Returning credit permits the nominal cadence;
                // do not divide a one-off pause by the number of ACKs and
                // carry that delay into otherwise healthy future frames.
            }
        }
        Some(self.last_capture.map_or(now, |last| last + self.period))
    }

    pub(crate) fn begin_capture(&mut self, now: Instant) {
        self.last_capture = Some(now);
    }

    /// Called immediately before sending compressed bytes, after capture has
    /// produced a frame. Register before the write so a fast ACK cannot race it.
    pub(crate) fn begin_send(&mut self, now: Instant) -> Result<u64> {
        if self.flights.len() >= DESKTOP_MAX_IN_FLIGHT as usize {
            return Err(DesktopError::Protocol("desktop video credit exhausted"));
        }
        self.sent = self
            .sent
            .checked_add(1)
            .ok_or(DesktopError::Protocol("frame ID overflow"))?;
        self.flights.push_back(Flight {
            id: self.sent,
            started: now,
        });
        Ok(self.sent)
    }

    pub(crate) fn acknowledge(&mut self, id: u64, now: Instant) -> Result<()> {
        if id == 0 || id < self.acknowledged || id > self.sent {
            return Err(DesktopError::reject(
                DesktopErrorCode::InvalidRequest,
                "invalid presentation acknowledgment",
            ));
        }
        if id == self.acknowledged {
            return Ok(());
        }
        while self.flights.front().is_some_and(|frame| frame.id <= id) {
            let frame = self.flights.pop_front().unwrap();
            // For a cumulative ACK use the newest acknowledged frame. The
            // others may already have been presented before this ACK arrived.
            self.presentation_ack_last_us = Some(now.saturating_duration_since(frame.started).as_micros().min(u64::MAX as u128) as u64);
        }
        let first_presentation = self.acknowledged == 0;
        self.acknowledged = id;
        if first_presentation {
            // The first presentation includes viewer/decoder startup. Keep
            // the bounded credit window during that wait, but do not treat
            // this one-time delay as the steady-state consumer capacity.
            // A slow peer will fill the window again and adapt normally.
            self.credit_wait_since = None;
        }
        let under_pressure = self.credit_wait_since.is_some_and(|since| {
            now.saturating_duration_since(since) > self.period.max(Duration::from_millis(100))
        });
        if under_pressure {
            self.quality.pressure(now);
        } else if !first_presentation {
            self.quality.consumed(now);
        }
        Ok(())
    }

    pub(crate) fn backlog_deadline(&self) -> Option<Instant> {
        self.flights
            .front()
            .map(|frame| frame.started + MAX_BACKLOG_AGE)
    }
    pub(crate) fn diagnostics(&self, now: Instant) -> (u64, u64, Option<u64>, Option<u64>) {
        (self.sent, self.acknowledged, self.presentation_ack_last_us,
            self.flights.front().map(|f| now.saturating_duration_since(f.started).as_millis().min(u64::MAX as u128) as u64))
    }
    pub(crate) fn encoding_budget(&self) -> u32 {
        self.quality.budget()
    }
    pub(crate) fn capture_fps_milli(&self) -> u32 {
        let nanos = self.period.as_nanos();
        ((1_000_000_000_000u128 + nanos / 2) / nanos) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restarting_preserves_capacity_without_reusing_old_frame_credit() {
        let now = Instant::now();
        let mut old = VideoFlow::new(15);
        old.quality.pressure(now);
        old.quality.pressure(now + Duration::from_millis(600));
        for _ in 0..DESKTOP_MAX_IN_FLIGHT { old.begin_send(now).unwrap(); }
        assert!(old.capture_at(now).is_none());
        let mut fresh = old.restart(30);
        assert_eq!(fresh.encoding_budget(), 3_000_000);
        assert_eq!(fresh.capture_fps_milli(), 30_000);
        assert_eq!(fresh.backlog_deadline(), None);
        assert_eq!(fresh.capture_at(now), Some(now));
        assert!(fresh.acknowledge(1, now).is_err());
        assert_eq!(fresh.begin_send(now).unwrap(), 1);
        fresh.acknowledge(1, now + Duration::from_millis(20)).unwrap();
        assert_eq!(fresh.encoding_budget(), 3_000_000);
        assert_eq!(fresh.period, Duration::from_secs_f64(1.0 / 30.0));
        assert_eq!(old.flights.len(), DESKTOP_MAX_IN_FLIGHT as usize);
    }

    #[test]
    fn nominal_rate_rounding_never_reports_a_false_quality_reduction() {
        for fps in 1..=60 {
            assert_eq!(VideoFlow::new(fps).capture_fps_milli(), u32::from(fps) * 1000);
        }
    }

    #[test]
    fn credit_duplicate_ack_and_future_ack_do_not_corrupt_history() {
        let now = Instant::now();
        let mut flow = VideoFlow::new(15);
        assert!(flow.acknowledge(1, now).is_err());
        for id in 1..=4 {
            assert_eq!(flow.begin_send(now).unwrap(), id);
        }
        assert!(flow.capture_at(now).is_none());
        assert!(flow.begin_send(now).is_err());
        flow.acknowledge(2, now + Duration::from_millis(50))
            .unwrap();
        let deadline = flow.backlog_deadline();
        flow.acknowledge(2, now + Duration::from_secs(4)).unwrap();
        assert_eq!(flow.backlog_deadline(), deadline);
        assert!(flow.acknowledge(1, now).is_err());
        assert!(flow.acknowledge(5, now).is_err());
        assert_eq!(flow.flights.len(), 2);
    }

    #[test]
    fn a_network_pause_returns_to_requested_cadence_without_a_catchup_burst() {
        let start = Instant::now();
        let mut now = start;
        let mut flow = VideoFlow::new(30);
        flow.begin_capture(now);
        flow.begin_send(now).unwrap();
        flow.acknowledge(1, now + Duration::from_millis(10)).unwrap();
        now += flow.period;
        for _ in 0..4 {
            flow.begin_capture(now);
            flow.begin_send(now).unwrap();
            now += flow.period;
        }
        assert!(flow.capture_at(now).is_none());
        now += Duration::from_millis(900);
        flow.acknowledge(5, now).unwrap();
        assert!(flow.capture_at(now).is_some());
        assert_eq!(flow.encoding_budget(), 0);
        assert_eq!(flow.capture_fps_milli(), 30_000);
        flow.begin_capture(now);
        flow.begin_send(now).unwrap();
        assert_eq!(flow.capture_at(now).unwrap(), now + flow.period);
    }

    #[test]
    fn startup_is_excluded_and_subsequent_slow_consumption_keeps_bounded_credit() {
        let start = Instant::now();
        let mut flow = VideoFlow::new(15);
        for n in 0..DESKTOP_MAX_IN_FLIGHT {
            let now = start + flow.period * n as u32;
            flow.begin_capture(now);
            flow.begin_send(now).unwrap();
        }
        let full = start + flow.period * DESKTOP_MAX_IN_FLIGHT as u32;
        assert!(flow.capture_at(full).is_none());
        assert!(flow.begin_send(full).is_err());
        let first = full + Duration::from_secs(2);
        flow.acknowledge(1, first).unwrap();
        assert!(flow.capture_at(first).unwrap() <= first);
        assert_eq!(flow.capture_fps_milli(), 15_000);
        flow.begin_capture(first);
        flow.begin_send(first).unwrap();
        assert!(flow.capture_at(first).is_none());
        let second = first + Duration::from_secs(1);
        flow.acknowledge(2, second).unwrap();
        assert!(flow.capture_at(second).is_some());
        assert_eq!(flow.capture_fps_milli(), 15_000);
        assert_eq!(flow.encoding_budget(), 0);
        assert!(flow.flights.len() < DESKTOP_MAX_IN_FLIGHT as usize);
        // A slow consumer can refill only the returned single credit. No new
        // source frame may be produced until another display ACK arrives.
        flow.begin_send(second).unwrap();
        assert!(flow.capture_at(second).is_none());
        assert!(flow.begin_send(second).is_err());
        let third = second + Duration::from_millis(600);
        flow.acknowledge(3, third).unwrap();
        assert!(flow.capture_at(third).is_some());
        assert_eq!(flow.encoding_budget(), 3_000_000);
    }

    #[test]
    fn static_capture_time_is_excluded_and_no_catchup_burst_is_allowed() {
        let start = Instant::now();
        let mut flow = VideoFlow::new(15);
        assert_eq!(flow.backlog_deadline(), None);
        flow.begin_capture(start);
        let ready = start + Duration::from_secs(7200);
        let id = flow.begin_send(ready).unwrap();
        assert_eq!(flow.backlog_deadline(), Some(ready + MAX_BACKLOG_AGE));
        flow.acknowledge(id, ready + Duration::from_millis(10))
            .unwrap();
        assert_eq!(flow.capture_fps_milli(), 15_000);
        assert_eq!(flow.backlog_deadline(), None);
        // One new request may start immediately after a long pending capture,
        // but cannot spend that idle time as credits for a burst of requests.
        assert!(flow.capture_at(ready).unwrap() <= ready);
        flow.begin_capture(ready);
        assert_eq!(flow.capture_at(ready).unwrap(), ready + flow.period);
    }
}
