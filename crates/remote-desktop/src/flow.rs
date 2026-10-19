//! Sender-clock feedback only: slow consumption delays returning capture
//! credit. Decoder pipeline latency alone must not reduce capture cadence.
use crate::{DesktopError, Result};
use pab_protocol::{DESKTOP_MAX_IN_FLIGHT, DesktopErrorCode};
use std::{collections::VecDeque, time::Duration};
use tokio::time::Instant;

pub(crate) const MAX_BACKLOG_AGE: Duration = Duration::from_secs(5);
const RECOVERY_INTERVAL: Duration = Duration::from_secs(1);

struct Flight {
    id: u64,
    started: Instant,
}

pub(crate) struct VideoFlow {
    sent: u64,
    acknowledged: u64,
    flights: VecDeque<Flight>,
    nominal_period: Duration,
    period: Duration,
    credit_wait_since: Option<Instant>,
    last_capture: Option<Instant>,
    last_adjustment: Option<Instant>,
    quality: crate::quality::EncodingQuality,
}

impl VideoFlow {
    pub(crate) fn new(fps: u16) -> Self {
        let period = Duration::from_secs_f64(1.0 / f64::from(fps));
        Self {
            sent: 0,
            acknowledged: 0,
            flights: VecDeque::with_capacity(DESKTOP_MAX_IN_FLIGHT as usize),
            nominal_period: period,
            period,
            credit_wait_since: None,
            last_capture: None,
            last_adjustment: None,
            quality: crate::quality::EncodingQuality::new(),
        }
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
                // A cumulative ACK may return several credits together. Avoid
                // attributing their whole wait to each individual next frame.
                let returned = DESKTOP_MAX_IN_FLIGHT as u32 - self.flights.len() as u32;
                let desired =
                    (waited / returned).clamp(self.nominal_period, Duration::from_secs(1));
                if desired > self.period {
                    self.period = desired;
                    self.last_adjustment = Some(now);
                }
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
            self.flights.pop_front();
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
        if !under_pressure
            && self.period > self.nominal_period
            && self
                .last_adjustment
                .is_none_or(|last| now - last >= RECOVERY_INTERVAL)
        {
            self.period = (self.period * 7 / 8).max(self.nominal_period);
            self.last_adjustment = Some(now);
        }
        Ok(())
    }

    pub(crate) fn backlog_deadline(&self) -> Option<Instant> {
        self.flights
            .front()
            .map(|frame| frame.started + MAX_BACKLOG_AGE)
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
    fn slow_consumption_reduces_capture_rate_then_recovers_gradually() {
        let start = Instant::now();
        let mut now = start;
        let mut flow = VideoFlow::new(30);
        flow.begin_capture(now);
        flow.begin_send(now).unwrap();
        flow.acknowledge(1, now + Duration::from_millis(10)).unwrap();
        now += flow.nominal_period;
        for _ in 0..4 {
            flow.begin_capture(now);
            flow.begin_send(now).unwrap();
            now += flow.nominal_period;
        }
        assert!(flow.capture_at(now).is_none());
        now += Duration::from_millis(900);
        flow.acknowledge(5, now).unwrap();
        assert!(flow.capture_at(now).is_some());
        assert_eq!(flow.period, Duration::from_millis(225));
        let slow_period = flow.period;
        for _ in 0..30 {
            now += Duration::from_secs(1);
            let id = flow.begin_send(now).unwrap();
            now += Duration::from_millis(10);
            flow.acknowledge(id, now).unwrap();
            assert!(flow.period <= slow_period && flow.period >= flow.nominal_period);
        }
        assert_eq!(flow.period, flow.nominal_period);
    }

    #[test]
    fn first_presentation_startup_does_not_throttle_but_subsequent_pressure_does() {
        let start = Instant::now();
        let mut flow = VideoFlow::new(15);
        for n in 0..DESKTOP_MAX_IN_FLIGHT {
            let now = start + flow.nominal_period * n as u32;
            flow.begin_capture(now);
            flow.begin_send(now).unwrap();
        }
        let full = start + flow.nominal_period * DESKTOP_MAX_IN_FLIGHT as u32;
        assert!(flow.capture_at(full).is_none());
        assert!(flow.begin_send(full).is_err());
        let first = full + Duration::from_secs(2);
        flow.acknowledge(1, first).unwrap();
        assert!(flow.capture_at(first).unwrap() <= first);
        assert_eq!(flow.period, flow.nominal_period);
        flow.begin_capture(first);
        flow.begin_send(first).unwrap();
        assert!(flow.capture_at(first).is_none());
        let second = first + Duration::from_secs(1);
        flow.acknowledge(2, second).unwrap();
        assert!(flow.capture_at(second).is_some());
        assert!(flow.period > flow.nominal_period);
        assert!(flow.flights.len() < DESKTOP_MAX_IN_FLIGHT as usize);
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
        assert_eq!(flow.period, flow.nominal_period);
        assert_eq!(flow.backlog_deadline(), None);
        // One new request may start immediately after a long pending capture,
        // but cannot spend that idle time as credits for a burst of requests.
        assert!(flow.capture_at(ready).unwrap() <= ready);
        flow.begin_capture(ready);
        assert_eq!(flow.capture_at(ready).unwrap(), ready + flow.nominal_period);
    }
}
