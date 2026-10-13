use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use pab_protocol::{DeviceRef, RequestId};
use serde::Serialize;
use tokio::{
    net::TcpListener,
    sync::{Mutex, OwnedSemaphorePermit, Semaphore, watch},
    task::{JoinHandle, JoinSet},
};

use super::{BridgeRuntime, RuntimeError};
use crate::AuthenticatedDeviceConnection;

const OPEN_TIMEOUT: Duration = Duration::from_secs(20);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_FORWARDS: usize = 8;
const MAX_CONNECTIONS: usize = 64;
const MAX_TOMBSTONES: usize = 128;
const TOMBSTONE_AGE: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Listening,
    Closing,
    Closed,
    Failed,
}

#[derive(Serialize)]
pub struct PortForwardStatus {
    pub forward_id: RequestId,
    pub device_ref: DeviceRef,
    pub local_host: &'static str,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    state: Phase,
    active_connections: u32,
    successful_connections: u64,
    failed_connections: u64,
    bytes_to_remote: u64,
    bytes_to_local: u64,
    connection_path: Option<&'static str>,
    last_error: Option<String>,
    last_error_at_unix_ms: Option<u64>,
}

struct EntryState {
    phase: Phase,
    last_error: Option<String>,
    last_error_at_unix_ms: Option<u64>,
    ended_at: Option<Instant>,
}

struct Entry {
    id: RequestId,
    device: DeviceRef,
    local_port: u16,
    requested_local_port: u16,
    host: String,
    port: u16,
    connection: Arc<AuthenticatedDeviceConnection>,
    state: StdMutex<EntryState>,
    active: AtomicU32,
    successful: AtomicU64,
    failed: AtomicU64,
    to_remote: AtomicU64,
    to_local: AtomicU64,
    close: watch::Sender<bool>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl Entry {
    fn state(&self) -> std::sync::MutexGuard<'_, EntryState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn status(&self) -> PortForwardStatus {
        let state = self.state();
        let connection_path = match self.connection.connection().selected_path() {
            Some(pab_transport::ConnectionPath::Direct) => Some("direct"),
            Some(pab_transport::ConnectionPath::Relay) => Some("relay"),
            Some(pab_transport::ConnectionPath::Unknown) => Some("unknown"),
            None => None,
        };
        PortForwardStatus {
            forward_id: self.id,
            device_ref: self.device,
            local_host: "127.0.0.1",
            local_port: self.local_port,
            remote_host: self.host.clone(),
            remote_port: self.port,
            state: state.phase,
            active_connections: self.active.load(Ordering::Relaxed),
            successful_connections: self.successful.load(Ordering::Relaxed),
            failed_connections: self.failed.load(Ordering::Relaxed),
            bytes_to_remote: self.to_remote.load(Ordering::Relaxed),
            bytes_to_local: self.to_local.load(Ordering::Relaxed),
            connection_path,
            last_error: state.last_error.clone(),
            last_error_at_unix_ms: state.last_error_at_unix_ms,
        }
    }

    fn fail_connection(&self, error: impl Into<String>) {
        self.failed.fetch_add(1, Ordering::Relaxed);
        let mut state = self.state();
        state.last_error = Some(error.into());
        state.last_error_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok());
    }

    fn finish(&self, phase: Phase, error: Option<String>) {
        let mut state = self.state();
        if matches!(state.phase, Phase::Closed | Phase::Failed) {
            return;
        }
        state.phase = phase;
        if let Some(error) = error {
            state.last_error = Some(error);
            state.last_error_at_unix_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok());
        }
        state.ended_at = Some(Instant::now());
    }
}

struct ActiveGuard {
    entry: Arc<Entry>,
    _permit: OwnedSemaphorePermit,
}
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.entry.active.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct ManagerState {
    entries: HashMap<RequestId, Arc<Entry>>,
    order: VecDeque<RequestId>,
}

impl ManagerState {
    fn prune(&mut self) {
        self.order.retain(|id| self.entries.contains_key(id));
        let mut closed_count = self
            .entries
            .values()
            .filter(|entry| entry.state().ended_at.is_some())
            .count();
        for id in self.order.iter().copied().collect::<Vec<_>>() {
            let Some(entry) = self.entries.get(&id) else {
                continue;
            };
            let ended = entry.state().ended_at;
            if let Some(ended) = ended {
                if ended.elapsed() >= TOMBSTONE_AGE || closed_count > MAX_TOMBSTONES {
                    self.entries.remove(&id);
                    closed_count -= 1;
                }
            }
        }
        self.order.retain(|id| self.entries.contains_key(id));
    }
}

pub(super) struct PortForwardManager {
    open_lock: Mutex<()>,
    state: Mutex<ManagerState>,
    slots: Arc<Semaphore>,
}

impl Default for PortForwardManager {
    fn default() -> Self {
        Self {
            open_lock: Mutex::new(()),
            state: Mutex::new(ManagerState::default()),
            slots: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        }
    }
}

impl BridgeRuntime {
    pub async fn port_forward_open(
        &self,
        device: DeviceRef,
        host: String,
        port: u16,
        local_port: u16,
    ) -> Result<PortForwardStatus, RuntimeError> {
        if port == 0 || !pab_protocol::valid_port_forward_host(&host) {
            return Err(RuntimeError::PortForward("invalid TCP target".into()));
        }
        let manager = &self.inner.port_forwards;
        let _open = manager.open_lock.lock().await;
        let stale = {
            let mut state = manager.state.lock().await;
            state.prune();
            state
                .entries
                .values()
                .find(|entry| {
                    entry.device == device
                        && entry.host == host
                        && entry.port == port
                        && entry.requested_local_port == local_port
                        && entry.state().phase == Phase::Listening
                })
                .cloned()
        };
        if let Some(existing) = stale {
            if existing.connection.connection().selected_path().is_some() {
                return Ok(existing.status());
            }
            existing.finish(Phase::Failed, Some("device_connection_closed".into()));
            let _ = manager.close(existing.id).await;
        }
        {
            let mut state = manager.state.lock().await;
            state.prune();
            if state
                .entries
                .values()
                .filter(|entry| matches!(entry.state().phase, Phase::Listening | Phase::Closing))
                .count()
                >= MAX_FORWARDS
            {
                return Err(RuntimeError::PortForward(
                    "resource_limit: eight active listeners per MCP process".into(),
                ));
            }
        }
        let deadline = tokio::time::Instant::now() + OPEN_TIMEOUT;
        let connection = tokio::time::timeout_at(deadline, async {
            self.current_environment(device).await?;
            self.inner.device(device).await.connection().await
        })
        .await
        .map_err(|_| {
            RuntimeError::PortForward("device_unavailable: authentication timed out".into())
        })??;
        let supported = tokio::time::timeout_at(deadline, connection.port_forward_supported())
            .await
            .map_err(|_| {
                RuntimeError::PortForward("device_unavailable: capability check timed out".into())
            })??;
        if !supported {
            return Err(RuntimeError::PortForward(
                "unsupported: update the target Executor".into(),
            ));
        }
        if connection.connection().selected_path().is_none() {
            return Err(RuntimeError::PortForward(
                "device connection closed before listening".into(),
            ));
        }
        let listener = TcpListener::bind(("127.0.0.1", local_port))
            .await
            .map_err(|error| RuntimeError::PortForward(format!("bind_failed: {error}")))?;
        let actual_port = listener
            .local_addr()
            .map_err(|error| RuntimeError::PortForward(error.to_string()))?
            .port();
        if connection.connection().selected_path().is_none() {
            return Err(RuntimeError::PortForward(
                "device connection closed while binding".into(),
            ));
        }
        let (close, receiver) = watch::channel(false);
        let entry = Arc::new(Entry {
            id: RequestId::new(),
            device,
            local_port: actual_port,
            requested_local_port: local_port,
            host,
            port,
            connection,
            state: StdMutex::new(EntryState {
                phase: Phase::Listening,
                last_error: None,
                last_error_at_unix_ms: None,
                ended_at: None,
            }),
            active: AtomicU32::new(0),
            successful: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            to_remote: AtomicU64::new(0),
            to_local: AtomicU64::new(0),
            close,
            task: Mutex::new(None),
        });
        {
            let mut state = manager.state.lock().await;
            state.order.push_back(entry.id);
            state.entries.insert(entry.id, entry.clone());
        }
        let running = entry.clone();
        let slots = manager.slots.clone();
        *entry.task.lock().await = Some(tokio::spawn(async move {
            run_listener(running, listener, receiver, slots).await
        }));
        if entry.connection.connection().selected_path().is_none() {
            entry.finish(Phase::Failed, Some("device_connection_closed".into()));
            let _ = manager.close(entry.id).await;
            return Err(RuntimeError::PortForward(
                "device connection closed before listening".into(),
            ));
        }
        Ok(entry.status())
    }

    pub async fn port_forward_status(
        &self,
        id: RequestId,
    ) -> Result<PortForwardStatus, RuntimeError> {
        let mut state = self.inner.port_forwards.state.lock().await;
        state.prune();
        state
            .entries
            .get(&id)
            .map(|entry| entry.status())
            .ok_or_else(|| {
                RuntimeError::PortForward("not_found: unknown or expired forward_id".into())
            })
    }

    pub async fn port_forward_close(
        &self,
        id: RequestId,
    ) -> Result<PortForwardStatus, RuntimeError> {
        self.inner.port_forwards.close(id).await
    }
}

impl PortForwardManager {
    async fn close(&self, id: RequestId) -> Result<PortForwardStatus, RuntimeError> {
        let entry = {
            let mut state = self.state.lock().await;
            state.prune();
            state.entries.get(&id).cloned()
        }
        .ok_or_else(|| {
            RuntimeError::PortForward("not_found: unknown or expired forward_id".into())
        })?;
        {
            let mut state = entry.state();
            if state.phase == Phase::Listening {
                state.phase = Phase::Closing;
            }
        }
        entry.close.send_replace(true);
        let mut task_slot = entry.task.lock().await;
        if let Some(task) = task_slot.take() {
            let _ = task.await;
        }
        drop(task_slot);
        Ok(entry.status())
    }

    pub(super) async fn close_all(&self) {
        let ids = self
            .state
            .lock()
            .await
            .entries
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for id in ids {
            let _ = self.close(id).await;
        }
    }
}

async fn run_listener(
    entry: Arc<Entry>,
    listener: TcpListener,
    mut close: watch::Receiver<bool>,
    slots: Arc<Semaphore>,
) {
    let mut children = JoinSet::new();
    let mut failure = None;
    loop {
        tokio::select! {
            _ = entry.connection.connection().closed() => {
                failure = Some("device_connection_closed".to_owned());
                break;
            }
            changed = close.changed() => {
                if changed.is_err() || *close.borrow() { break; }
            }
            accepted = listener.accept() => match accepted {
                Ok((tcp, _)) => {
                    if *close.borrow() { break; }
                    match slots.clone().try_acquire_owned() {
                        Ok(permit) => {
                            entry.active.fetch_add(1, Ordering::Relaxed);
                            let child = entry.clone();
                            children.spawn(async move {
                                let _guard = ActiveGuard { entry: child.clone(), _permit: permit };
                                match child.connection.port_forward_connect(&child.host, child.port).await {
                                    Ok(stream) => {
                                    match stream.relay_tcp(tcp, &child.to_remote, &child.to_local, IDLE_TIMEOUT).await {
                                        Ok(()) => { child.successful.fetch_add(1, Ordering::Relaxed); }
                                        Err(error) => child.fail_connection(format!("relay_failed: {error}")),
                                    }
                                    }
                                    Err(error) => child.fail_connection(format!("connect_failed: {error}")),
                                }
                            });
                        }
                        Err(_) => entry.fail_connection("resource_limit: local TCP relay limit reached"),
                    }
                }
                Err(error) => { failure = Some(format!("listener_failed: {error}")); break; }
            },
            Some(joined) = children.join_next(), if !children.is_empty() => {
                if let Err(error) = joined { entry.fail_connection(format!("relay_task_failed: {error}")); }
            }
        }
    }
    drop(listener);
    children.abort_all();
    while children.join_next().await.is_some() {}
    if let Some(error) = failure {
        entry.finish(Phase::Failed, Some(error));
    } else {
        entry.finish(Phase::Closed, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pab_protocol::{DeviceId, DeviceTaskRequest, DeviceTaskResponse, TenantId};
    use pab_transport::{PabConnection, PabEndpoint, PabEndpointAddress, PabEndpointConfig};
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn pair() -> (PabEndpoint, PabEndpoint, PabConnection, PabConnection) {
        let config = PabEndpointConfig::new(vec!["https://127.0.0.1:1".parse().unwrap()]).unwrap();
        let first_key: [u8; 32] = Sha256::digest(RequestId::new().to_string().as_bytes()).into();
        let second_key: [u8; 32] = Sha256::digest(RequestId::new().to_string().as_bytes()).into();
        let first = PabEndpoint::bind(config.clone(), iroh_base::SecretKey::from_bytes(&first_key))
            .await
            .unwrap();
        let second = PabEndpoint::bind(config, iroh_base::SecretKey::from_bytes(&second_key))
            .await
            .unwrap();
        let mut addresses = second.watch_address();
        tokio::time::timeout(Duration::from_secs(5), async {
            while addresses.borrow().direct_addresses.is_empty() {
                addresses.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        let direct_addresses = addresses
            .borrow()
            .direct_addresses
            .iter()
            .map(|address| {
                std::net::SocketAddr::new(
                    if address.is_ipv4() {
                        std::net::Ipv4Addr::LOCALHOST.into()
                    } else {
                        std::net::Ipv6Addr::LOCALHOST.into()
                    },
                    address.port(),
                )
            })
            .collect();
        let accepting = second.clone();
        let remote = tokio::spawn(async move { accepting.accept().await.unwrap().unwrap() });
        let client = first
            .connect(
                *second.id().as_bytes(),
                &PabEndpointAddress {
                    relay_urls: vec![],
                    direct_addresses,
                },
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        (first, second, client, remote.await.unwrap())
    }

    #[tokio::test]
    async fn listener_forwards_and_close_releases_port() {
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_port = target.local_addr().unwrap().port();
        let target_task = tokio::spawn(async move {
            let (mut socket, _) = target.accept().await.unwrap();
            let mut data = Vec::new();
            socket.read_to_end(&mut data).await.unwrap();
            socket.write_all(&data).await.unwrap();
            socket.shutdown().await.unwrap();
            data
        });
        let (first, second, client, server) = pair().await;
        let remote = server.clone();
        let remote_task = tokio::spawn(async move {
            let mut stream = remote.accept_bi(Duration::from_secs(5)).await.unwrap();
            let request: DeviceTaskRequest =
                stream.receive_json(Duration::from_secs(5)).await.unwrap();
            let DeviceTaskRequest::PortForwardConnect { host, port, .. } = request else {
                panic!("wrong request")
            };
            let tcp = tokio::net::TcpStream::connect((host.as_str(), port))
                .await
                .unwrap();
            stream
                .send_frame_json(
                    &DeviceTaskResponse::PortForwardConnected,
                    Duration::from_secs(5),
                )
                .await
                .unwrap();
            stream
                .relay_tcp(
                    tcp,
                    &AtomicU64::new(0),
                    &AtomicU64::new(0),
                    Duration::from_secs(5),
                )
                .await
                .unwrap();
        });
        let device = DeviceRef {
            tenant_id: TenantId::from_u128(1),
            device_id: DeviceId::from_u128(2),
        };
        let connection = Arc::new(AuthenticatedDeviceConnection::test_new(client, device));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_port = listener.local_addr().unwrap().port();
        let (close, receiver) = watch::channel(false);
        let id = RequestId::new();
        let entry = Arc::new(Entry {
            id,
            device,
            local_port,
            requested_local_port: 0,
            host: "127.0.0.1".into(),
            port: target_port,
            connection,
            state: StdMutex::new(EntryState {
                phase: Phase::Listening,
                last_error: None,
                last_error_at_unix_ms: None,
                ended_at: None,
            }),
            active: AtomicU32::new(0),
            successful: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            to_remote: AtomicU64::new(0),
            to_local: AtomicU64::new(0),
            close,
            task: Mutex::new(None),
        });
        let manager = PortForwardManager::default();
        manager.state.lock().await.entries.insert(id, entry.clone());
        let running = entry.clone();
        let slots = manager.slots.clone();
        *entry.task.lock().await = Some(tokio::spawn(async move {
            run_listener(running, listener, receiver, slots).await
        }));
        let mut app = tokio::net::TcpStream::connect(("127.0.0.1", local_port))
            .await
            .unwrap();
        let payload = (0..70_000_u32).map(|n| (n % 251) as u8).collect::<Vec<_>>();
        app.write_all(&payload).await.unwrap();
        app.shutdown().await.unwrap();
        let mut reply = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), app.read_to_end(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert!(reply == payload, "listener changed TCP payload");
        assert!(
            target_task.await.unwrap() == payload,
            "target received different bytes"
        );
        remote_task.await.unwrap();
        let status = manager.close(id).await.unwrap();
        assert_eq!(status.state, Phase::Closed);
        assert_eq!(status.active_connections, 0);
        assert_eq!(status.successful_connections, 1);
        assert_eq!(status.failed_connections, 0);
        assert_eq!(status.bytes_to_remote, payload.len() as u64);
        assert_eq!(status.bytes_to_local, payload.len() as u64);
        assert!(
            TcpListener::bind(("127.0.0.1", local_port)).await.is_ok(),
            "close left the port bound"
        );
        assert_eq!(manager.close(id).await.unwrap().state, Phase::Closed);
        drop((first, second, server));
    }
}
