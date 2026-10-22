use iroh_base::SecretKey;
use pab_protocol::{
    ControlErrorCode, ControlServerMessage, DeviceHello, DeviceNetworkUpdate,
    EndpointProofPrincipal, RequestId,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
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
        duration_millis, publish, publish_stopped,
    },
    device_network_resolver::{
        DeviceNetworkLookup, DeviceNetworkRequest, DeviceNetworkResolver, ResolvedDevice,
    },
    peer_authorizer::{DevicePeerAuthorizer, PeerAuthorizationRequest},
};

pub struct EndpointControlSupervisor {
    config: EndpointControlConfig,
    secret: SecretKey,
    connector: Connector,
    policy: ReconnectPolicy,
    device_hello: Option<DeviceHello>,
    optional_device_hello: bool,
    device_network: Option<watch::Receiver<DeviceNetworkUpdate>>,
    relay_directory_enabled: bool,
    relay_cache: Option<crate::relay_cache::RelayCache>,
    relay_seeds: Vec<String>,
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
            optional_device_hello: false,
            device_network: None,
            relay_directory_enabled: false,
            relay_cache: None,
            relay_seeds: Vec::new(),
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
        if update.device_ref.tenant_id != self.config.tenant_id
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
        if hello.device_ref.tenant_id != self.config.tenant_id
            || hello.device_ref.device_id != device_id
        {
            return Err(DeviceHelloConfigError::IdentityMismatch);
        }
        self.device_hello = Some(hello);
        Ok(self)
    }

    /// Older control servers cannot decode new platform metadata. Opt-in endpoints
    /// may reconnect without that optional announcement after a mismatched response.
    /// Authentication, signed identity and network publication remain mandatory.
    /// Existing callers keep the strict behavior unless they explicitly opt in.
    pub fn with_optional_device_hello(mut self) -> Self {
        self.optional_device_hello = true;
        self
    }

    pub fn with_relay_directory(mut self) -> Self {
        self.relay_directory_enabled = true;
        self
    }

    /// Cache is bound to the exact control origin and endpoint key. It only
    /// supplies reconnect candidates; authorization still comes from Server.
    pub fn with_relay_directory_cache(
        mut self,
        directory: &std::path::Path,
        seeds: Vec<String>,
    ) -> Self {
        let scope =
            blake3::hash(format!("{}:{}", self.config.url, self.secret.public()).as_bytes())
                .to_hex()
                .to_string();
        self.relay_cache = Some(crate::relay_cache::RelayCache {
            path: directory.join(format!("relay-directory-{scope}.json")),
            scope,
        });
        self.relay_seeds = seeds;
        self.relay_directory_enabled = true;
        self
    }

    pub fn spawn(self) -> EndpointControlSupervisorHandle {
        let (status_sender, status) = watch::channel(ControlConnectionStatus::initial());
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let (relay_updates, relay_receiver) = watch::channel((0, Vec::new()));
        let relay_updates = self.relay_directory_enabled.then_some(relay_updates);
        let (peer_requests, peer_request_receiver) = mpsc::channel(32);
        let peer_authorizer =
            DevicePeerAuthorizer::new(peer_requests, self.config.operation_timeout);
        let (network_requests, network_request_receiver) = mpsc::channel(32);
        let device_network_resolver =
            DeviceNetworkResolver::new(network_requests, self.config.operation_timeout);
        let task = tokio::spawn(self.run(
            status_sender,
            shutdown_receiver,
            peer_request_receiver,
            network_request_receiver,
            relay_updates,
        ));
        EndpointControlSupervisorHandle {
            status,
            shutdown,
            peer_authorizer,
            device_network_resolver,
            relay_updates: relay_receiver,
            task,
        }
    }

    async fn run(
        mut self,
        status: watch::Sender<ControlConnectionStatus>,
        mut shutdown: watch::Receiver<bool>,
        mut peer_requests: mpsc::Receiver<PeerAuthorizationRequest>,
        mut network_requests: mpsc::Receiver<DeviceNetworkRequest>,
        relay_updates: Option<watch::Sender<(u64, Vec<String>)>>,
    ) {
        let mut generation = 0_u64;
        let mut consecutive_failures = 0_u32;
        if let (Some(cache), Some(sender)) = (&self.relay_cache, &relay_updates) {
            let cache = cache.clone();
            if let Ok(Some(urls)) = tokio::task::spawn_blocking(move || cache.load()).await {
                sender.send_replace((1, urls));
            }
        }
        let cache_writer = self.relay_cache.clone().map(|cache| cache.writer());
        let mut directory_revision = 0;
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
                    if self.optional_device_hello
                        && matches!(&hello_result, Err(EndpointControlError::MismatchedResponse))
                    {
                        // Use a fresh authenticated connection. Never reuse a stream
                        // whose reply could not be bound to the metadata request.
                        self.device_hello = None;
                        self.optional_device_hello = false;
                    }
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
                            let mut requests = ControlRequestReceivers {
                                peer: &mut peer_requests,
                                network: &mut network_requests,
                            };
                            match maintain_connection(
                                &mut connection,
                                generation,
                                self.policy,
                                &mut shutdown,
                                &mut self.device_network,
                                &mut requests,
                                self.config.operation_timeout,
                                relay_updates.as_ref(),
                                *self.secret.public().as_bytes(),
                                &mut directory_revision,
                                cache_writer.as_ref(),
                                &self.relay_seeds,
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
            let delay = self.policy.retry_delay();
            publish(
                &status,
                ControlConnectionPhase::Reconnecting,
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
    peer_authorizer: DevicePeerAuthorizer,
    device_network_resolver: DeviceNetworkResolver,
    task: JoinHandle<()>,
    relay_updates: watch::Receiver<(u64, Vec<String>)>,
}

impl EndpointControlSupervisorHandle {
    pub fn relay_updates(&self) -> watch::Receiver<(u64, Vec<String>)> {
        self.relay_updates.clone()
    }
    pub fn status(&self) -> watch::Receiver<ControlConnectionStatus> {
        self.status.clone()
    }

    pub fn peer_authorizer(&self) -> DevicePeerAuthorizer {
        self.peer_authorizer.clone()
    }

    pub fn device_network_resolver(&self) -> DeviceNetworkResolver {
        self.device_network_resolver.clone()
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
    requests: &mut ControlRequestReceivers<'_>,
    operation_timeout: std::time::Duration,
    relay_updates: Option<&watch::Sender<(u64, Vec<String>)>>,
    relay_identity: [u8; 32],
    directory_revision: &mut u64,
    cache_writer: Option<&watch::Sender<Option<Vec<String>>>>,
    relay_seeds: &[String],
) -> Result<(), EndpointControlError> {
    let start = Instant::now() + policy.heartbeat_interval;
    let mut heartbeat = tokio::time::interval_at(start, policy.heartbeat_interval);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut sequence = 0_u64;
    let mut pending_heartbeat: Option<(Vec<u8>, Instant)> = None;
    let mut pending_network: Option<PendingNetworkUpdate> = None;
    let mut pending_peer: Option<PendingPeerAuthorization> = None;
    let mut pending_resolution: Option<PendingDeviceNetworkResolution> = None;
    let mut directory_poll = tokio::time::interval(std::time::Duration::from_secs(5));
    directory_poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut directory_supported = relay_updates.is_some();
    let mut pending_directory: Option<(RequestId, Instant)> = None;
    loop {
        tokio::select! {
            _ = directory_poll.tick(), if directory_supported && pending_directory.is_none() => {
                let id = connection.send_relay_directory(operation_timeout).await?;
                pending_directory = Some((id, Instant::now()+operation_timeout));
            }
            _ = wait_for_deadline(pending_directory.as_ref().map(|(_,deadline)| *deadline)) => {
                return Err(EndpointControlError::Timeout);
            }
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
                        match &message {
                            ControlServerMessage::RelayDirectory { request_id, directory }
                                if pending_directory.as_ref().is_some_and(|(id,_)| id==request_id) => {
                                pending_directory = None;
                                if let (Some(directory),Some(sender)) = (directory,relay_updates) {
                                    directory.validate().map_err(|_| EndpointControlError::UnexpectedMessage)?;
                                    let urls=ranked_relay_urls(directory,&relay_identity);
                                    if directory.revision>*directory_revision{
                                        *directory_revision=directory.revision;
                                        sender.send_modify(|current|*current=(current.0.saturating_add(1),urls.clone()));
                                    }
                                    // Refresh age even when the catalog is unchanged; a
                                    // long-lived healthy process must not leave an expired
                                    // on-disk cache for the next restart.
                                    if directory.revision==*directory_revision{
                                        if let Some(cache)=cache_writer{cache.send_replace(Some(urls));}
                                    }
                                }else if let Some(sender)=relay_updates{
                                    // A deliberate rollback to single-node mode restores configured seeds.
                                    *directory_revision=0;
                                    if !relay_seeds.is_empty(){sender.send_modify(|current|*current=(current.0.saturating_add(1),relay_seeds.to_vec()));}
                                    if let Some(cache)=cache_writer{cache.send_replace(Some(relay_seeds.to_vec()));}
                                }
                                continue;
                            }
                            ControlServerMessage::Error { request_id,code:ControlErrorCode::InvalidMessage,.. }
                                if pending_directory.as_ref().is_some_and(|(id,_)| request_id.is_none_or(|received| received==*id)) => {
                                // Old servers reject the optional message and remain usable.
                                pending_directory = None;
                                directory_supported = false;
                                *directory_revision=0;
                                if !relay_seeds.is_empty(){
                                    if let Some(sender)=relay_updates{sender.send_modify(|current|*current=(current.0.saturating_add(1),relay_seeds.to_vec()));}
                                    if let Some(cache)=cache_writer{cache.send_replace(Some(relay_seeds.to_vec()));}
                                }
                                continue;
                            }
                            _ => {}
                        }
                        handle_server_message(
                            message,
                            &mut pending_network,
                            &mut pending_peer,
                            &mut pending_resolution,
                        )?;
                    }
                }
            }
            request = requests.peer.recv(), if pending_peer.is_none() => {
                let Some(request) = request else {
                    return Ok(());
                };
                let request_id = connection
                    .send_authorize_device_peer(
                        request.peer_endpoint_key,
                        request.authenticated,
                        operation_timeout,
                    )
                    .await?;
                pending_peer = Some(PendingPeerAuthorization {
                    request_id,
                    peer_endpoint_key: request.peer_endpoint_key,
                    response: request.response,
                    deadline: Instant::now() + operation_timeout,
                });
            }
            request = requests.network.recv(), if pending_resolution.is_none() => {
                let Some(request) = request else {
                    return Ok(());
                };
                let request_id = match request.lookup {
                    DeviceNetworkLookup::Ref(device_ref) => connection
                        .send_get_device_network(device_ref, operation_timeout)
                        .await?,
                    DeviceNetworkLookup::Code(device_code) => connection
                        .send_resolve_device_code(device_code, operation_timeout)
                        .await?,
                    DeviceNetworkLookup::Presence(device_code) => connection
                        .send_get_device_presence(device_code, operation_timeout)
                        .await?,
                    DeviceNetworkLookup::List => connection
                        .send_list_devices(operation_timeout)
                        .await?,
                };
                pending_resolution = Some(PendingDeviceNetworkResolution {
                    request_id,
                    lookup: request.lookup,
                    response: request.response,
                    deadline: Instant::now() + operation_timeout,
                });
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
            _ = wait_for_deadline(pending_peer.as_ref().map(|pending| pending.deadline)) => {
                return Err(EndpointControlError::Timeout);
            }
            _ = wait_for_deadline(pending_resolution.as_ref().map(|pending| pending.deadline)) => {
                return Err(EndpointControlError::Timeout);
            }
        }
    }
}

struct ControlRequestReceivers<'a> {
    peer: &'a mut mpsc::Receiver<PeerAuthorizationRequest>,
    network: &'a mut mpsc::Receiver<DeviceNetworkRequest>,
}

struct PendingNetworkUpdate {
    request_id: RequestId,
    update: DeviceNetworkUpdate,
    deadline: Instant,
}

struct PendingPeerAuthorization {
    request_id: RequestId,
    peer_endpoint_key: pab_protocol::EndpointKey,
    response: oneshot::Sender<Result<pab_protocol::AuthorizedDevicePeer, EndpointControlError>>,
    deadline: Instant,
}

struct PendingDeviceNetworkResolution {
    request_id: RequestId,
    lookup: DeviceNetworkLookup,
    response: oneshot::Sender<Result<ResolvedDevice, EndpointControlError>>,
    deadline: Instant,
}

fn handle_server_message(
    message: ControlServerMessage,
    pending_network: &mut Option<PendingNetworkUpdate>,
    pending_peer: &mut Option<PendingPeerAuthorization>,
    pending_resolution: &mut Option<PendingDeviceNetworkResolution>,
) -> Result<(), EndpointControlError> {
    match message {
        ControlServerMessage::DeviceNetworkAccepted { request_id, result }
            if pending_network.as_ref().is_some_and(|pending| {
                request_id == pending.request_id
                    && result.device_ref == pending.update.device_ref
                    && result.endpoint_instance_id == pending.update.endpoint_instance_id
                    && result.address_revision == pending.update.address_revision
            }) =>
        {
            *pending_network = None;
            Ok(())
        }
        ControlServerMessage::DevicePeerAuthorized { request_id, result }
            if pending_peer.as_ref().is_some_and(|pending| {
                request_id == pending.request_id
                    && result.peer_endpoint_key == pending.peer_endpoint_key
            }) =>
        {
            let pending = pending_peer.take().expect("matched peer request");
            let _ = pending.response.send(Ok(result));
            Ok(())
        }
        ControlServerMessage::DeviceNetworkFound {
            request_id,
            snapshot,
        } if pending_resolution.as_ref().is_some_and(|pending| {
            request_id == pending.request_id
                && matches!(pending.lookup, DeviceNetworkLookup::Ref(device_ref) if snapshot.device_ref == device_ref)
        }) =>
        {
            let pending = pending_resolution.take().expect("matched network request");
            let _ = pending.response.send(Ok(ResolvedDevice::Network(*snapshot)));
            Ok(())
        }
        ControlServerMessage::DeviceCodeResolved {
            request_id,
            device_ref,
        } if pending_resolution.as_ref().is_some_and(|pending| {
            request_id == pending.request_id && matches!(pending.lookup, DeviceNetworkLookup::Code(_))
        }) =>
        {
            let pending = pending_resolution.take().expect("matched code request");
            let _ = pending.response.send(Ok(ResolvedDevice::Ref(device_ref)));
            Ok(())
        }
        ControlServerMessage::DeviceList {
            request_id,
            devices,
        } if pending_resolution.as_ref().is_some_and(|pending| {
            request_id == pending.request_id
                && matches!(pending.lookup, DeviceNetworkLookup::List)
        }) => {
            let pending = pending_resolution.take().expect("matched list request");
            let _ = pending.response.send(Ok(ResolvedDevice::List(devices)));
            Ok(())
        }
        ControlServerMessage::DevicePresenceFound {
            request_id,
            presence,
        } if pending_resolution.as_ref().is_some_and(|pending| {
            request_id == pending.request_id
                && matches!(pending.lookup, DeviceNetworkLookup::Presence(code) if code == presence.code)
        }) => {
            let pending = pending_resolution.take().expect("matched presence request");
            let _ = pending.response.send(Ok(ResolvedDevice::Presence(presence)));
            Ok(())
        }
        ControlServerMessage::Error {
            request_id: Some(request_id),
            code,
            message,
        } if pending_network
            .as_ref()
            .is_some_and(|pending| request_id == pending.request_id) =>
        {
            Err(EndpointControlError::Server { code, message })
        }
        ControlServerMessage::Error {
            request_id: Some(request_id),
            code,
            message,
        } if pending_peer
            .as_ref()
            .is_some_and(|pending| request_id == pending.request_id) =>
        {
            let pending = pending_peer.take().expect("matched peer request");
            let _ = pending
                .response
                .send(Err(EndpointControlError::Server { code, message }));
            Ok(())
        }
        ControlServerMessage::Error {
            request_id: Some(request_id),
            code,
            message,
        } if pending_resolution
            .as_ref()
            .is_some_and(|pending| request_id == pending.request_id) =>
        {
            let pending = pending_resolution.take().expect("matched network request");
            let _ = pending
                .response
                .send(Err(EndpointControlError::Server { code, message }));
            Ok(())
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

fn ranked_relay_urls(directory: &pab_protocol::RelayDirectory, identity: &[u8; 32]) -> Vec<String> {
    let mut nodes: Vec<_> = directory
        .nodes
        .iter()
        .filter(|node| node.accepts_new_connections())
        .map(|node| {
            let mut hash = blake3::Hasher::new();
            hash.update(identity);
            hash.update(node.node_id.as_bytes());
            let bytes = hash.finalize();
            let score = u64::from_le_bytes(bytes.as_bytes()[..8].try_into().unwrap());
            let uniform = (score as f64 + 1.0) / (u64::MAX as f64 + 1.0);
            // Routing weights are explicit; an unspecified host capacity is not
            // a measured network speed and must not bias placement.
            let rank = -uniform.ln() / f64::from(node.weight);
            (rank, node.url.clone())
        })
        .collect();
    nodes.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    nodes.into_iter().map(|(_, url)| url).collect()
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use iroh_base::SecretKey;
    use pab_protocol::{EndpointProofPrincipal, TenantId, UserId};

    use super::*;

    #[test]
    fn node_ranking_is_stable_weighted_and_excludes_maintenance() {
        use pab_protocol::{RelayAdminState, RelayDirectory, RelayDirectoryNode, RelayHealth};
        let node = |id: &str, weight| RelayDirectoryNode {
            node_id: id.into(),
            url: format!("https://{id}.test/"),
            region: "test".into(),
            weight,
            capacity_mbps: 100,
            admin_state: RelayAdminState::Active,
            health: RelayHealth::Healthy,
        };
        let mut directory = RelayDirectory {
            schema_version: 1,
            revision: 1,
            nodes: vec![node("a", 1), node("b", 3)],
        };
        let mut b_count = 0;
        for i in 0u64..2000 {
            let identity = *blake3::hash(&i.to_le_bytes()).as_bytes();
            let ranked = ranked_relay_urls(&directory, &identity);
            directory.nodes.reverse();
            assert_eq!(ranked, ranked_relay_urls(&directory, &identity));
            if ranked[0] == "https://b.test/" {
                b_count += 1;
            }
        }
        assert!(
            (1350..1650).contains(&b_count),
            "weighted distribution: {b_count}"
        );
        directory
            .nodes
            .iter_mut()
            .find(|n| n.node_id == "b")
            .unwrap()
            .admin_state = RelayAdminState::Maintenance;
        assert_eq!(ranked_relay_urls(&directory, &[0; 32]), ["https://a.test/"]);
    }

    #[tokio::test]
    async fn retries_forever_at_the_same_interval() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let supervisor = EndpointControlSupervisor::new(
            EndpointControlConfig {
                url: format!("wss://127.0.0.1:{port}/control"),
                tenant_id: TenantId::from_u128(2),
                principal: EndpointProofPrincipal::User {
                    user_id: UserId::from_u128(3),
                },
                operation_timeout: Duration::from_millis(100),
            },
            SecretKey::generate(),
            crate::tls_connector(None).unwrap(),
            ReconnectPolicy {
                heartbeat_interval: Duration::from_secs(1),
                heartbeat_timeout: Duration::from_millis(500),
                retry_interval: Duration::from_millis(25),
            },
        )
        .unwrap()
        .spawn();
        let mut status = supervisor.status();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let current = status.borrow().clone();
                if current.phase == ControlConnectionPhase::Reconnecting {
                    assert_eq!(current.retry_in_ms, Some(25));
                    if current.consecutive_failures >= 4 {
                        break;
                    }
                }
                status.changed().await.unwrap();
            }
        })
        .await
        .expect("the supervisor stopped retrying");
        supervisor.shutdown().await.unwrap();
    }
}
