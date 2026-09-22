use std::time::{Duration, SystemTime, UNIX_EPOCH};

use thiserror::Error;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectPolicy {
    pub heartbeat_interval: Duration,
    pub heartbeat_timeout: Duration,
    pub initial_delay: Duration,
    pub max_delay: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(30),
            heartbeat_timeout: Duration::from_secs(10),
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl ReconnectPolicy {
    pub fn validate(self) -> Result<Self, ReconnectPolicyError> {
        if self.heartbeat_interval.is_zero() {
            return Err(ReconnectPolicyError::ZeroHeartbeatInterval);
        }
        if self.heartbeat_timeout.is_zero() {
            return Err(ReconnectPolicyError::ZeroHeartbeatTimeout);
        }
        if self.heartbeat_timeout >= self.heartbeat_interval {
            return Err(ReconnectPolicyError::HeartbeatTimeoutNotShorter);
        }
        if self.initial_delay.is_zero() {
            return Err(ReconnectPolicyError::ZeroInitialDelay);
        }
        if self.max_delay < self.initial_delay {
            return Err(ReconnectPolicyError::InvalidMaximumDelay);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlConnectionPhase {
    Disconnected,
    Connecting,
    Authenticated,
    BackingOff,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionFailureKind {
    Configuration,
    Authentication,
    Network,
    Timeout,
    Protocol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionFailure {
    pub kind: ConnectionFailureKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlConnectionStatus {
    pub phase: ControlConnectionPhase,
    pub generation: u64,
    pub consecutive_failures: u32,
    pub retry_in_ms: Option<u64>,
    pub changed_at_unix_ms: i64,
    pub last_failure: Option<ConnectionFailure>,
}

impl ControlConnectionStatus {
    pub(crate) fn initial() -> Self {
        Self {
            phase: ControlConnectionPhase::Disconnected,
            generation: 0,
            consecutive_failures: 0,
            retry_in_ms: None,
            changed_at_unix_ms: now_unix_millis(),
            last_failure: None,
        }
    }
}

pub(crate) fn reconnect_delay(
    policy: ReconnectPolicy,
    endpoint_key: [u8; 32],
    failures: u32,
) -> Duration {
    let exponent = failures.saturating_sub(1).min(31);
    let ceiling = policy
        .initial_delay
        .saturating_mul(1_u32 << exponent)
        .min(policy.max_delay);
    let ceiling_ms = duration_millis(ceiling);
    let floor_ms = ceiling_ms / 2;
    let spread = ceiling_ms.saturating_sub(floor_ms);
    let mut seed = u64::from_be_bytes(endpoint_key[..8].try_into().expect("fixed slice"))
        ^ u64::from(failures);
    seed ^= seed << 13;
    seed ^= seed >> 7;
    seed ^= seed << 17;
    let jitter = if spread == 0 { 0 } else { seed % (spread + 1) };
    Duration::from_millis(floor_ms + jitter)
}

pub(crate) fn duration_millis(value: Duration) -> u64 {
    u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn publish(
    status: &watch::Sender<ControlConnectionStatus>,
    phase: ControlConnectionPhase,
    generation: u64,
    consecutive_failures: u32,
    retry_in_ms: Option<u64>,
    last_failure: Option<ConnectionFailure>,
) {
    status.send_replace(ControlConnectionStatus {
        phase,
        generation,
        consecutive_failures,
        retry_in_ms,
        changed_at_unix_ms: now_unix_millis(),
        last_failure,
    });
}

pub(crate) fn publish_stopped(
    status: &watch::Sender<ControlConnectionStatus>,
    generation: u64,
    consecutive_failures: u32,
    last_failure: Option<ConnectionFailure>,
) {
    publish(
        status,
        ControlConnectionPhase::Stopped,
        generation,
        consecutive_failures,
        None,
        last_failure,
    );
}

fn now_unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|value| i64::try_from(value.as_millis()).ok())
        .unwrap_or(0)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReconnectPolicyError {
    #[error("heartbeat interval must be greater than zero")]
    ZeroHeartbeatInterval,
    #[error("heartbeat timeout must be greater than zero")]
    ZeroHeartbeatTimeout,
    #[error("heartbeat timeout must be shorter than the heartbeat interval")]
    HeartbeatTimeoutNotShorter,
    #[error("initial reconnect delay must be greater than zero")]
    ZeroInitialDelay,
    #[error("maximum reconnect delay must be at least the initial delay")]
    InvalidMaximumDelay,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DeviceHelloConfigError {
    #[error("device hello requires a device endpoint principal")]
    DevicePrincipalRequired,
    #[error("device hello identity does not match endpoint control configuration")]
    IdentityMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_heartbeat_and_reconnect_ranges() {
        let error = ReconnectPolicy {
            heartbeat_interval: Duration::from_secs(5),
            heartbeat_timeout: Duration::from_secs(5),
            ..ReconnectPolicy::default()
        }
        .validate()
        .unwrap_err();
        assert_eq!(error, ReconnectPolicyError::HeartbeatTimeoutNotShorter);
    }

    #[test]
    fn reconnect_delay_stays_within_the_jittered_exponential_window() {
        let policy = ReconnectPolicy {
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(800),
            ..ReconnectPolicy::default()
        };
        let endpoint = [9; 32];
        for (failures, ceiling_ms) in [(1, 100), (2, 200), (3, 400), (4, 800), (8, 800)] {
            let delay = reconnect_delay(policy, endpoint, failures);
            assert!(delay >= Duration::from_millis(ceiling_ms / 2));
            assert!(delay <= Duration::from_millis(ceiling_ms));
        }
    }
}
