use iroh_base::SecretKey;
use pab_protocol::{ControlErrorCode, DeviceHello, EndpointProofPrincipal};
use tokio::{
    sync::watch,
    task::{JoinError, JoinHandle},
    time::{Instant, MissedTickBehavior},
};
use tokio_tungstenite::Connector;

use crate::{
    AuthenticatedControlConnection, EndpointControlConfig, EndpointControlError,
    connection_state::{
        ConnectionFailure, ConnectionFailureKind, ControlConnectionPhase, ControlConnectionStatus,
        DeviceHelloConfigError, ReconnectPolicy, ReconnectPolicyError, duration_millis, publish,
        publish_stopped, reconnect_delay,
    },
};

pub struct EndpointControlSupervisor {
    config: EndpointControlConfig,
    secret: SecretKey,
    connector: Connector,
    policy: ReconnectPolicy,
    device_hello: Option<DeviceHello>,
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
            device_hello: None,
        })
    }

    pub fn with_device_hello(mut self, hello: DeviceHello) -> Result<Self, DeviceHelloConfigError> {
        let EndpointProofPrincipal::Device { device_id } = self.config.principal else {
            return Err(DeviceHelloConfigError::DevicePrincipalRequired);
        };
        if hello.device_ref.deployment_id != self.config.deployment_id
            || hello.device_ref.tenant_id != self.config.tenant_id
            || hello.device_ref.device_id != device_id
        {
            return Err(DeviceHelloConfigError::IdentityMismatch);
        }
        self.device_hello = Some(hello);
        Ok(self)
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
                    let hello_result = match self.device_hello.as_ref() {
                        Some(hello) => connection
                            .publish_device_hello(hello, self.config.operation_timeout)
                            .await
                            .map(|_| ()),
                        None => Ok(()),
                    };
                    match hello_result {
                        Err(error) => classify(error),
                        Ok(()) => {
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
    let kind = match &error {
        EndpointControlError::TlsRequired
        | EndpointControlError::InvalidTimeout
        | EndpointControlError::InvalidSystemTime => ConnectionFailureKind::Configuration,
        EndpointControlError::ChallengeMismatch
        | EndpointControlError::InvalidChallenge
        | EndpointControlError::IdentityMismatch => ConnectionFailureKind::Authentication,
        EndpointControlError::Server {
            code:
                ControlErrorCode::InvalidCredentials
                | ControlErrorCode::PermissionDenied
                | ControlErrorCode::NotFound,
            ..
        } => ConnectionFailureKind::Authentication,
        EndpointControlError::Server { .. } => ConnectionFailureKind::Protocol,
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
