use std::time::{Duration, SystemTime, UNIX_EPOCH};

use thiserror::Error;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectPolicy {
    pub heartbeat_interval: Duration,
    pub heartbeat_timeout: Duration,
    pub retry_interval: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(30),
            heartbeat_timeout: Duration::from_secs(10),
            retry_interval: Duration::from_secs(3),
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
        if self.retry_interval.is_zero() {
            return Err(ReconnectPolicyError::ZeroRetryInterval);
        }
        Ok(self)
    }

    pub const fn retry_delay(self) -> Duration {
        self.retry_interval
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlConnectionPhase {
    Disconnected,
    Connecting,
    Authenticated,
    Reconnecting,
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
    #[error("reconnect retry interval must be greater than zero")]
    ZeroRetryInterval,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DeviceHelloConfigError {
    #[error("device hello requires a device endpoint principal")]
    DevicePrincipalRequired,
    #[error("device hello identity does not match endpoint control configuration")]
    IdentityMismatch,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DeviceNetworkConfigError {
    #[error("device network updates require a device endpoint principal")]
    DevicePrincipalRequired,
    #[error("device network identity does not match endpoint control configuration")]
    IdentityMismatch,
    #[error("device network endpoint key does not match the control endpoint key")]
    EndpointKeyMismatch,
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
    fn reconnect_delay_is_fixed_after_every_failure() {
        let policy = ReconnectPolicy {
            retry_interval: Duration::from_millis(100),
            ..ReconnectPolicy::default()
        };
        assert_eq!(policy.retry_delay(), Duration::from_millis(100));
    }
}
