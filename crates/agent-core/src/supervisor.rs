use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iroh_base::SecretKey;
use thiserror::Error;
use tokio::{
    sync::watch,
    task::{JoinError, JoinHandle},
    time::{Instant, MissedTickBehavior},
};
use tokio_tungstenite::Connector;

use crate::{AuthenticatedControlConnection, EndpointControlConfig, EndpointControlError};

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
    fn initial() -> Self {
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

pub struct EndpointControlSupervisor {
    config: EndpointControlConfig,
    secret: SecretKey,
    connector: Connector,
    policy: ReconnectPolicy,
}

impl EndpointControlSupervisor {
    pub fn new(
        config: EndpointControlConfig,
        secret: SecretKey,
        connector: Connector,
        policy: ReconnectPolicy,
    ) -> Result<Self, ReconnectPolicyError> {
        Ok(Self {
            config,
            secret,
            connector,
            policy: policy.validate()?,
        })
    }

    pub fn spawn(self) -> EndpointControlSupervisorHandle {
        let (status_sender, status) = watch::channel(ControlConnectionStatus::initial());
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let task = tokio::spawn(self.run(status_sender, shutdown_receiver));
        EndpointControlSupervisorHandle {
            status,
            shutdown,
            task,
        }
    }

    async fn run(
        self,
        status: watch::Sender<ControlConnectionStatus>,
        mut shutdown: watch::Receiver<bool>,
    ) {
        let mut generation = 0_u64;
        let mut consecutive_failures = 0_u32;
        let endpoint_key = *self.secret.public().as_bytes();
        loop {
            if *shutdown.borrow() {
                publish_stopped(&status, generation, consecutive_failures, None);
                return;
            }
            let previous_failure = status.borrow().last_failure.clone();
            publish(
                &status,
                ControlConnectionPhase::Connecting,
                generation,
                consecutive_failures,
                None,
                previous_failure,
            );
            let connection = AuthenticatedControlConnection::connect(
                &self.config,
                &self.secret,
                self.connector.clone(),
            )
            .await;
            let failure = match connection {
                Ok(mut connection) => {
                    generation = generation.saturating_add(1);
                    consecutive_failures = 0;
                    publish(
                        &status,
                        ControlConnectionPhase::Authenticated,
                        generation,
                        0,
                        None,
                        None,
                    );
                    match maintain_connection(
                        &mut connection,
                        generation,
                        self.policy,
                        &mut shutdown,
                    )
                    .await
                    {
                        Ok(()) => {
                            publish_stopped(&status, generation, 0, None);
                            return;
                        }
                        Err(error) => classify(error),
                    }
                }
                Err(error) => classify(error),
            };
            consecutive_failures = consecutive_failures.saturating_add(1);
            if failure.kind == ConnectionFailureKind::Configuration {
                publish_stopped(&status, generation, consecutive_failures, Some(failure));
                return;
            }
            let delay = reconnect_delay(self.policy, endpoint_key, consecutive_failures);
            publish(
                &status,
                ControlConnectionPhase::BackingOff,
                generation,
                consecutive_failures,
                Some(duration_millis(delay)),
                Some(failure),
            );
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        publish_stopped(&status, generation, consecutive_failures, None);
                        return;
                    }
                }
            }
        }
    }
}

pub struct EndpointControlSupervisorHandle {
    status: watch::Receiver<ControlConnectionStatus>,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl EndpointControlSupervisorHandle {
    pub fn status(&self) -> watch::Receiver<ControlConnectionStatus> {
        self.status.clone()
    }

    pub async fn shutdown(self) -> Result<(), JoinError> {
        let _ = self.shutdown.send(true);
        self.task.await
    }
}

async fn maintain_connection(
    connection: &mut AuthenticatedControlConnection,
    generation: u64,
    policy: ReconnectPolicy,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), EndpointControlError> {
    let start = Instant::now() + policy.heartbeat_interval;
    let mut heartbeat = tokio::time::interval_at(start, policy.heartbeat_interval);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut sequence = 0_u64;
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                sequence = sequence.saturating_add(1);
                let mut payload = Vec::with_capacity(16);
                payload.extend_from_slice(&generation.to_be_bytes());
                payload.extend_from_slice(&sequence.to_be_bytes());
                connection.heartbeat(payload, policy.heartbeat_timeout).await?;
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

fn classify(error: EndpointControlError) -> ConnectionFailure {
    let kind = match error {
        EndpointControlError::TlsRequired
        | EndpointControlError::InvalidTimeout
        | EndpointControlError::InvalidSystemTime => ConnectionFailureKind::Configuration,
        EndpointControlError::ChallengeMismatch
        | EndpointControlError::InvalidChallenge
        | EndpointControlError::IdentityMismatch
        | EndpointControlError::Server { .. } => ConnectionFailureKind::Authentication,
        EndpointControlError::Timeout => ConnectionFailureKind::Timeout,
        EndpointControlError::Closed | EndpointControlError::WebSocket(_) => {
            ConnectionFailureKind::Network
        }
        EndpointControlError::UnexpectedMessage
        | EndpointControlError::MismatchedResponse
        | EndpointControlError::Json(_) => ConnectionFailureKind::Protocol,
    };
    ConnectionFailure {
        kind,
        detail: error.to_string(),
    }
}

fn reconnect_delay(policy: ReconnectPolicy, endpoint_key: [u8; 32], failures: u32) -> Duration {
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

fn duration_millis(value: Duration) -> u64 {
    u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
}

fn now_unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|value| i64::try_from(value.as_millis()).ok())
        .unwrap_or(0)
}

fn publish(
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

fn publish_stopped(
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
