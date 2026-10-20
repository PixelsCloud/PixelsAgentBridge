use super::*;
use pab_protocol::{DEVICE_TASK_SCHEMA_VERSION, DeviceId, DeviceRef, DeviceTaskRequest, TenantId};
use pab_transport::{PabConnection, PabEndpoint, PabEndpointAddress, PabEndpointConfig};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

struct Fixture {
    _root: tempfile::TempDir,
    _endpoints: (PabEndpoint, PabEndpoint),
    client: PabConnection,
    server: PabConnection,
    service: Arc<PortForwardService>,
    paused: Arc<AtomicBool>,
    gate: JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let execution = ExecutionService::open(
            &root.path().join("execution.sqlite"),
            [3; 32],
            DeviceRef {
                tenant_id: TenantId::new(),
                device_id: DeviceId::new(),
            },
        )
        .await
        .unwrap();
        let paused = Arc::new(AtomicBool::new(false));
        let state = paused.clone();
        let directory = root.path().to_owned();
        let gate = tokio::spawn({
            let execution = execution.clone();
            async move {
                loop {
                    let job = execution.sdk.poll().await;
                    if job.is_null() {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        continue;
                    }
                    let response = if state.load(Ordering::SeqCst) {
                        serde_json::json!({"error":"control_paused"})
                    } else {
                        serde_json::json!({"workspace":directory,"uid":12345})
                    };
                    execution
                        .sdk
                        .complete(
                            serde_json::from_value(job["ticket"].clone()).unwrap(),
                            response,
                        )
                        .await;
                }
            }
        });
        let config = PabEndpointConfig::new(vec!["https://127.0.0.1:1".parse().unwrap()]).unwrap();
        let key = || *uuid::Uuid::new_v4().as_bytes();
        let first = PabEndpoint::bind(
            config.clone(),
            iroh_base::SecretKey::from_bytes(&[key(), key()].concat().try_into().unwrap()),
        )
        .await
        .unwrap();
        let second = PabEndpoint::bind(
            config,
            iroh_base::SecretKey::from_bytes(&[key(), key()].concat().try_into().unwrap()),
        )
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
        Self {
            _root: root,
            _endpoints: (first, second),
            client,
            server: remote.await.unwrap(),
            service: PortForwardService::new(execution),
            paused,
            gate,
        }
    }

    async fn open(
        &self,
        slots: Arc<Semaphore>,
        host: &str,
        port: u16,
        version: u16,
    ) -> (
        DeviceTaskResponse,
        PabBiStream,
        JoinHandle<Result<(), PabConnectionError>>,
    ) {
        let service = self.service.clone();
        let server = self.server.clone();
        let task = tokio::spawn(async move {
            let mut stream = server.accept_bi(Duration::from_secs(5)).await?;
            let request: DeviceTaskRequest = stream.receive_json(Duration::from_secs(5)).await?;
            let DeviceTaskRequest::PortForwardConnect {
                host,
                port,
                port_forward_schema_version,
                ..
            } = request
            else {
                panic!("wrong request")
            };
            service
                .serve(
                    stream,
                    slots,
                    port_forward_schema_version,
                    &host,
                    port,
                    Duration::from_secs(5),
                )
                .await
        });
        let mut stream = self.client.open_bi(Duration::from_secs(5)).await.unwrap();
        stream
            .send_frame_json(
                &DeviceTaskRequest::PortForwardConnect {
                    schema_version: DEVICE_TASK_SCHEMA_VERSION,
                    port_forward_schema_version: version,
                    host: host.into(),
                    port,
                },
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        let reply = stream.receive_json(Duration::from_secs(5)).await.unwrap();
        (reply, stream, task)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.gate.abort();
        self.client.close(b"test complete");
        self.server.close(b"test complete");
    }
}

async fn app(stream: PabBiStream) -> (TcpStream, JoinHandle<Result<(), PabConnectionError>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (tcp, _) = listener.accept().await.unwrap();
    let task = tokio::spawn(async move {
        stream
            .relay_tcp(
                tcp,
                &AtomicU64::new(0),
                &AtomicU64::new(0),
                Duration::from_secs(5),
            )
            .await
    });
    (socket, task)
}

#[tokio::test]
async fn binary_half_close_and_pause_close_only_owned_relay() {
    let fixture = Fixture::new().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let target = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        socket.read_to_end(&mut bytes).await.unwrap();
        socket.write_all(&bytes).await.unwrap();
        socket.shutdown().await.unwrap();
        let (mut second, _) = listener.accept().await.unwrap();
        let mut byte = [0];
        // This independent destination is not controlled by ExecutionService.
        tokio::time::timeout(Duration::from_secs(5), second.read(&mut byte))
            .await
            .unwrap()
            .unwrap()
    });
    let slots = Arc::new(Semaphore::new(4));
    let (reply, stream, remote) = fixture.open(slots.clone(), "127.0.0.1", port, 1).await;
    assert_eq!(reply, DeviceTaskResponse::PortForwardConnected);
    let (mut socket, local) = app(stream).await;
    let bytes = (0..150_000).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    socket.write_all(&bytes).await.unwrap();
    socket.shutdown().await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bytes, response);
    local.await.unwrap().unwrap();
    remote.await.unwrap().unwrap();
    let (reply, _stream, remote) = fixture.open(slots.clone(), "localhost", port, 1).await;
    assert_eq!(reply, DeviceTaskResponse::PortForwardConnected);
    fixture.paused.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(4), remote)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(target.await.unwrap(), 0);
    assert_eq!(slots.available_permits(), 4);
    assert_eq!(fixture.service.slots.available_permits(), 8);
    let (reply, _, remote) = fixture.open(slots, "127.0.0.1", port, 1).await;
    assert!(matches!(
        reply,
        DeviceTaskResponse::Error {
            code: pab_protocol::DeviceTaskErrorCode::AccessDenied,
            ..
        }
    ));
    remote.await.unwrap().unwrap();
}

#[tokio::test]
async fn version_target_limits_and_refusal_do_not_break_connection() {
    let fixture = Fixture::new().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    for (version, host, slots, code) in [
        (2, "127.0.0.1", 4, PortForwardErrorCode::Unsupported),
        (1, "https://bad", 4, PortForwardErrorCode::InvalidTarget),
        (1, "127.0.0.1", 0, PortForwardErrorCode::ResourceLimit),
        (1, "127.0.0.1", 4, PortForwardErrorCode::ConnectionRefused),
    ] {
        let (reply, _, remote) = fixture
            .open(Arc::new(Semaphore::new(slots)), host, port, version)
            .await;
        assert!(
            matches!(reply,DeviceTaskResponse::PortForwardRejected {code:actual,..} if actual==code),
            "{reply:?}"
        );
        remote.await.unwrap().unwrap();
        assert_eq!(fixture.service.slots.available_permits(), 8);
    }
    let global = fixture
        .service
        .slots
        .clone()
        .acquire_many_owned(8)
        .await
        .unwrap();
    let (reply, _, remote) = fixture
        .open(Arc::new(Semaphore::new(4)), "127.0.0.1", port, 1)
        .await;
    assert!(matches!(
        reply,
        DeviceTaskResponse::PortForwardRejected {
            code: PortForwardErrorCode::ResourceLimit,
            ..
        }
    ));
    remote.await.unwrap().unwrap();
    drop(global);
    assert_eq!(fixture.service.slots.available_permits(), 8);
    assert!(fixture.client.selected_path().is_some());
}

#[tokio::test]
async fn transport_close_releases_destination_and_both_slot_budgets() {
    let fixture = Fixture::new().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let slots = Arc::new(Semaphore::new(1));
    let (reply, _stream, remote) = fixture
        .open(
            slots.clone(),
            "127.0.0.1",
            listener.local_addr().unwrap().port(),
            1,
        )
        .await;
    assert_eq!(reply, DeviceTaskResponse::PortForwardConnected);
    let (mut target, _) = listener.accept().await.unwrap();
    let (reply, _, rejected) = fixture
        .open(
            slots.clone(),
            "127.0.0.1",
            listener.local_addr().unwrap().port(),
            1,
        )
        .await;
    assert!(matches!(
        reply,
        DeviceTaskResponse::PortForwardRejected {
            code: PortForwardErrorCode::ResourceLimit,
            ..
        }
    ));
    rejected.await.unwrap().unwrap();
    fixture.client.close(b"disconnect");
    assert!(
        tokio::time::timeout(Duration::from_secs(5), remote)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    let mut byte = [0];
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), target.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    assert_eq!(slots.available_permits(), 1);
    assert_eq!(fixture.service.slots.available_permits(), 8);
}
