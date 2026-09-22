use iroh_base::SecretKey;
use pab_protocol::{
    ControlErrorCode, ControlServerMessage, DeviceHello, DeviceNetworkUpdate,
    EndpointProofPrincipal, RequestId,
};
use tokio::{
    sync::watch,
    task::{JoinError, JoinHandle},
    time::{Instant, MissedTickBehavior},
};
use tokio_tungstenite::Connector;

use crate::{
    AuthenticatedControlConnection, EndpointControlConfig, EndpointControlError,
    connection_io::IncomingControlFrame,
    connection_state::{
        ConnectionFailure, ConnectionFailureKind, ControlConnectionPhase, ControlConnectionStatus,
        DeviceHelloConfigError, DeviceNetworkConfigError, ReconnectPolicy, ReconnectPolicyError,
        duration_millis, publish, publish_stopped, reconnect_delay,
    },
};

pub struct EndpointControlSupervisor {
    config: EndpointControlConfig,
    secret: SecretKey,
    connector: Connector,
    policy: ReconnectPolicy,
    device_hello: Option<DeviceHello>,
    device_network: Option<watch::Receiver<DeviceNetworkUpdate>>,
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
            device_network: None,
        })
    }

    pub fn with_device_network(
        mut self,
        updates: watch::Receiver<DeviceNetworkUpdate>,
    ) -> Result<Self, DeviceNetworkConfigError> {
        let EndpointProofPrincipal::Device { device_id } = self.config.principal else {
            return Err(DeviceNetworkConfigError::DevicePrincipalRequired);
        };
        let update = updates.borrow();
        if update.device_ref.deployment_id != self.config.deployment_id
            || update.device_ref.tenant_id != self.config.tenant_id
            || update.device_ref.device_id != device_id
        {
            return Err(DeviceNetworkConfigError::IdentityMismatch);
        }
        if update.endpoint_key.as_bytes() != self.secret.public().as_bytes() {
            return Err(DeviceNetworkConfigError::EndpointKeyMismatch);
        }
        drop(update);
        self.device_network = Some(updates);
        Ok(self)
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
        mut self,
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
                    let network_result = match (hello_result, self.device_network.as_mut()) {
                        (Ok(()), Some(updates)) => {
                            let update = updates.borrow_and_update().clone();
                            connection
                                .publish_device_network(&update, self.config.operation_timeout)
                                .await
                                .map(|_| ())
                        }
                        (result, _) => result,
                    };
                    match network_result {
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
                                &mut self.device_network,
                                self.config.operation_timeout,
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
    device_network: &mut Option<watch::Receiver<DeviceNetworkUpdate>>,
    operation_timeout: std::time::Duration,
) -> Result<(), EndpointControlError> {
    let start = Instant::now() + policy.heartbeat_interval;
    let mut heartbeat = tokio::time::interval_at(start, policy.heartbeat_interval);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut sequence = 0_u64;
    let mut pending_heartbeat: Option<(Vec<u8>, Instant)> = None;
    let mut pending_network: Option<PendingNetworkUpdate> = None;
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if pending_heartbeat.is_none() {
                    sequence = sequence.saturating_add(1);
                    let mut payload = Vec::with_capacity(16);
                    payload.extend_from_slice(&generation.to_be_bytes());
                    payload.extend_from_slice(&sequence.to_be_bytes());
                    connection
                        .send_ping(payload.clone(), policy.heartbeat_timeout)
                        .await?;
                    pending_heartbeat = Some((payload, Instant::now() + policy.heartbeat_timeout));
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            frame = connection.next_frame() => {
                match frame? {
                    IncomingControlFrame::Pong(received) => {
                        if pending_heartbeat.as_ref().is_some_and(|(sent, _)| *sent == received) {
                            pending_heartbeat = None;
                        }
                    }
                    IncomingControlFrame::Server(message) => {
                        handle_server_message(message, &mut pending_network)?;
                    }
                }
            }
            changed = network_changed(device_network, pending_network.is_none()) => {
                match changed {
                    Ok(()) => {
                        let update = device_network
                            .as_mut()
                            .expect("enabled network receiver")
                            .borrow_and_update()
                            .clone();
                        let request_id = connection
                            .send_device_network(&update, operation_timeout)
                            .await?;
                        pending_network = Some(PendingNetworkUpdate {
                            request_id,
                            update,
                            deadline: Instant::now() + operation_timeout,
                        });
                    }
                    Err(_) => *device_network = None,
                }
            }
            _ = wait_for_deadline(heartbeat_deadline(&pending_heartbeat)) => {
                return Err(EndpointControlError::Timeout);
            }
            _ = wait_for_deadline(pending_network.as_ref().map(|pending| pending.deadline)) => {
                return Err(EndpointControlError::Timeout);
            }
        }
    }
}

struct PendingNetworkUpdate {
    request_id: RequestId,
    update: DeviceNetworkUpdate,
    deadline: Instant,
}

fn handle_server_message(
    message: ControlServerMessage,
    pending: &mut Option<PendingNetworkUpdate>,
) -> Result<(), EndpointControlError> {
    match message {
        ControlServerMessage::DeviceNetworkAccepted { request_id, result }
            if pending.as_ref().is_some_and(|pending| {
                request_id == pending.request_id
                    && result.device_ref == pending.update.device_ref
                    && result.endpoint_instance_id == pending.update.endpoint_instance_id
                    && result.address_revision == pending.update.address_revision
            }) =>
        {
            *pending = None;
            Ok(())
        }
        ControlServerMessage::Error {
            request_id: Some(request_id),
            code,
            message,
        } if pending
            .as_ref()
            .is_some_and(|pending| request_id == pending.request_id) =>
        {
            Err(EndpointControlError::Server { code, message })
        }
        _ => Err(EndpointControlError::UnexpectedMessage),
    }
}

async fn network_changed(
    receiver: &mut Option<watch::Receiver<DeviceNetworkUpdate>>,
    enabled: bool,
) -> Result<(), watch::error::RecvError> {
    if enabled && let Some(receiver) = receiver {
        receiver.changed().await
    } else {
        std::future::pending().await
    }
}

fn heartbeat_deadline(pending: &Option<(Vec<u8>, Instant)>) -> Option<Instant> {
    pending.as_ref().map(|(_, deadline)| *deadline)
}

async fn wait_for_deadline(deadline: Option<Instant>) {
    if let Some(deadline) = deadline {
        tokio::time::sleep_until(deadline).await;
    } else {
        std::future::pending().await
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
