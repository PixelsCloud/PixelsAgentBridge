use std::time::{Duration, Instant};

// A new login desktop can take much longer than six fast inventory failures.
// Bound recovery by elapsed time, without replaying input or retrying permanent
// errors. The caller clears this clock only after stable viewing or user retry.
const RECOVERY_WINDOW: Duration = Duration::from_secs(120);

// A control NotFound includes an offline device during reboot. Retry it only
// after this viewer has actually displayed a frame, within the same deadline
// as transport/media recovery. Initial lookup failures and auth rejections
// retain their existing behavior; no general connection policy changes.
pub(super) fn can_retry(recoverable: bool, device_not_found: bool, has_presented: bool) -> bool {
    recoverable || (device_not_found && has_presented)
}

pub(super) fn retry_delay(
    first_failure: &mut Option<Instant>,
    failures: u32,
    now: Instant,
) -> Option<Duration> {
    let started = *first_failure.get_or_insert(now);
    let remaining = RECOVERY_WINDOW.checked_sub(now.saturating_duration_since(started))?;
    if remaining.is_zero() {
        return None;
    }
    Some(Duration::from_millis(250u64 << failures.min(3)).min(remaining))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_recovery_requires_a_previously_presented_frame() {
        assert!(!can_retry(false, true, false));
        assert!(can_retry(false, true, true));
        // Auth, permission and other permanent failures never inherit the
        // offline exception, even after a successful viewing session.
        assert!(!can_retry(false, false, true));
        assert!(!can_retry(false, false, false));
        assert!(can_retry(true, false, false));
    }

    #[test]
    fn delayed_login_is_not_abandoned_after_six_fast_failures() {
        let start = Instant::now();
        let mut first = None;
        for attempt in 0..40 {
            let now = start + Duration::from_secs(u64::from(attempt) * 2);
            assert!(retry_delay(&mut first, attempt, now).is_some());
        }
        assert_eq!(first, Some(start));
        assert!(retry_delay(&mut first, 40, start + RECOVERY_WINDOW).is_none());
    }

    #[test]
    fn slow_or_repeated_failures_do_not_extend_the_deadline() {
        let start = Instant::now();
        let mut first = Some(start);
        assert_eq!(
            retry_delay(&mut first, 2, start + Duration::from_millis(119_900)),
            Some(Duration::from_millis(100))
        );
        assert!(retry_delay(&mut first, 3, start + Duration::from_secs(121)).is_none());
        assert_eq!(first, Some(start));
    }

    #[test]
    fn backoff_is_bounded_even_for_large_attempt_counts() {
        let start = Instant::now();
        let mut first = None;
        assert_eq!(
            retry_delay(&mut first, 0, start),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            retry_delay(&mut first, u32::MAX, start),
            Some(Duration::from_secs(2))
        );
    }
}
