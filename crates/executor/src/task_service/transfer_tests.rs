//! Actual binary-frame/QUIC tests against an isolated TaskService and temporary
//! files. Authentication is represented by a verified fixture actor; production
//! endpoints, registrations and processes are never used or interrupted.
use super::*;
use pab_protocol::{DeploymentId, DeviceId, RequestId, TenantId, UserId};
use pab_transport::{PabConnection, PabEndpoint, PabEndpointAddress, PabEndpointConfig};

const TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) async fn pair() -> (PabEndpoint, PabEndpoint, PabConnection, PabConnection) {
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
    tokio::time::timeout(TIMEOUT, async {
        while addresses.borrow().direct_addresses.is_empty() {
            addresses.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let direct = addresses
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
    let remote = second.clone();
    let accepted = tokio::spawn(async move { remote.accept().await.unwrap().unwrap() });
    let client = first
        .connect(
            *second.id().as_bytes(),
            &PabEndpointAddress {
                relay_urls: vec![],
                direct_addresses: direct,
            },
            TIMEOUT,
        )
        .await
        .unwrap();
    let server = accepted.await.unwrap();
    (first, second, client, server)
}

pub(crate) fn actor() -> OperatorRef {
    OperatorRef::account(
        UserId::from_u128(81),
        pab_protocol::EndpointKey::new([81; 32]),
    )
}

pub(crate) async fn service(path: &Path) -> TaskService {
    TaskService::open(
        &path.join("executor.sqlite3"),
        DeviceRef {
            deployment_id: DeploymentId::from_u128(1),
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        },
        pab_platform::detect_native_execution_context().unwrap(),
    )
    .await
    .unwrap()
}

async fn receiver(
    service: &TaskService,
    connection: &PabConnection,
) -> tokio::task::JoinHandle<Result<(), TaskServiceError>> {
    let service = service.clone();
    let connection = connection.clone();
    tokio::spawn(async move {
        let stream = connection.accept_bi(TIMEOUT).await?;
        service.handle_stream(actor(), stream, TIMEOUT).await
    })
}

async fn upload(
    connection: &PabConnection,
    id: RequestId,
    path: &Path,
    bytes: &[u8],
) -> pab_transport::PabBiStream {
    let mut stream = connection.open_bi(TIMEOUT).await.unwrap();
    stream
        .send_frame_json(
            &DeviceTaskRequest::UploadFile {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id: id,
                path: path.to_string_lossy().into_owned(),
                size: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(bytes)),
                overwrite: true,
            },
            TIMEOUT,
        )
        .await
        .unwrap();
    assert!(matches!(
        stream
            .receive_json::<DeviceTaskResponse>(TIMEOUT)
            .await
            .unwrap(),
        DeviceTaskResponse::FileReady { offset: 0, .. }
    ));
    stream
}

#[tokio::test]
async fn binary_upload_publication_survives_lost_receipt_and_overwrites_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let service = service(directory.path()).await;
    let (first, second, client, server) = pair().await;
    let destination = directory.path().join("中文 result.bin");
    tokio::fs::write(&destination, b"previous").await.unwrap();
    let bytes: Vec<u8> = (0..131071).map(|i| (i % 256) as u8).collect();
    let id = RequestId::new();
    let worker = receiver(&service, &server).await;
    let mut stream = upload(&client, id, &destination, &bytes).await;
    assert!(stream.send_binary_frame(&[], TIMEOUT).await.is_err());
    assert!(
        stream
            .send_binary_frame(&vec![0; pab_transport::MAX_BINARY_FRAME_BYTES + 1], TIMEOUT)
            .await
            .is_err()
    );
    let mut offset = 0;
    for chunk in bytes.chunks(65536) {
        stream.send_binary_frame(chunk, TIMEOUT).await.unwrap();
        offset += chunk.len() as u64;
        assert!(
            matches!(stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap(),DeviceTaskResponse::FileProgress { offset: received } if received == offset)
        );
        if offset < bytes.len() as u64 {
            assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"previous");
        }
    }
    // Stop receiving before FileComplete. The final file and durable record
    // must still agree even though the receipt cannot reach the caller.
    drop(stream);
    let _ = tokio::time::timeout(TIMEOUT, worker)
        .await
        .unwrap()
        .unwrap();
    let snapshot = service.store.get_transfer(actor(), id).await.unwrap();
    assert_eq!(snapshot.state, "completed");
    assert_eq!(snapshot.published, Some(true));
    assert_eq!(tokio::fs::read(&destination).await.unwrap(), bytes);
    first.close().await;
    second.close().await;
}

#[tokio::test]
async fn interrupted_binary_upload_leaves_the_existing_file_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let service = service(directory.path()).await;
    let (first, second, client, server) = pair().await;
    let destination = directory.path().join("result.bin");
    tokio::fs::write(&destination, b"previous").await.unwrap();
    let bytes = vec![255; 131072];
    let id = RequestId::new();
    let worker = receiver(&service, &server).await;
    let mut stream = upload(&client, id, &destination, &bytes).await;
    stream
        .send_binary_frame(&bytes[..65536], TIMEOUT)
        .await
        .unwrap();
    let _: DeviceTaskResponse = stream.receive_json(TIMEOUT).await.unwrap();
    drop(stream);
    assert!(
        tokio::time::timeout(TIMEOUT, worker)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    let snapshot = service.store.get_transfer(actor(), id).await.unwrap();
    assert_eq!(snapshot.state, "failed");
    assert_eq!(snapshot.offset, 65536);
    assert_eq!(snapshot.published, Some(false));
    assert_eq!(tokio::fs::read(destination).await.unwrap(), b"previous");
    first.close().await;
    second.close().await;
}

#[tokio::test]
async fn conflicting_upload_is_rejected_without_crossing_binary_streams() {
    let directory = tempfile::tempdir().unwrap();
    let service = service(directory.path()).await;
    let (first, second, client, server) = pair().await;
    let destination = directory.path().join("result.bin");
    let bytes = b"fixture";
    let id = RequestId::new();
    let worker = receiver(&service, &server).await;
    let mut stream = upload(&client, id, &destination, bytes).await;
    let conflict_id = RequestId::new();
    let conflicting_worker = receiver(&service, &server).await;
    let mut conflict = client.open_bi(TIMEOUT).await.unwrap();
    conflict
        .send_frame_json(
            &DeviceTaskRequest::UploadFile {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id: conflict_id,
                path: destination.to_string_lossy().into_owned(),
                size: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(bytes)),
                overwrite: true,
            },
            TIMEOUT,
        )
        .await
        .unwrap();
    assert!(matches!(
        conflict
            .receive_json::<DeviceTaskResponse>(TIMEOUT)
            .await
            .unwrap(),
        DeviceTaskResponse::Error { .. }
    ));
    assert!(conflicting_worker.await.unwrap().is_err());
    assert_eq!(
        service
            .store
            .get_transfer(actor(), conflict_id)
            .await
            .unwrap()
            .published,
        Some(false)
    );
    stream.send_binary_frame(bytes, TIMEOUT).await.unwrap();
    let _: DeviceTaskResponse = stream.receive_json(TIMEOUT).await.unwrap();
    assert!(matches!(
        stream
            .receive_json::<DeviceTaskResponse>(TIMEOUT)
            .await
            .unwrap(),
        DeviceTaskResponse::FileComplete { .. }
    ));
    worker.await.unwrap().unwrap();
    assert_eq!(tokio::fs::read(destination).await.unwrap(), bytes);
    first.close().await;
    second.close().await;
}

#[tokio::test]
async fn empty_and_frame_boundary_sized_files_round_trip_as_binary() {
    let directory = tempfile::tempdir().unwrap();
    let service = service(directory.path()).await;
    let (first, second, client, server) = pair().await;
    let destination = directory.path().join("boundary.bin");
    for size in [0, 1, 65535, 65536, 65537, 262143, 262144, 262145] {
        let bytes: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
        let id = RequestId::new();
        let worker = receiver(&service, &server).await;
        let mut stream = upload(&client, id, &destination, &bytes).await;
        for chunk in bytes.chunks(65536) {
            stream.send_binary_frame(chunk, TIMEOUT).await.unwrap();
            assert!(matches!(
                stream
                    .receive_json::<DeviceTaskResponse>(TIMEOUT)
                    .await
                    .unwrap(),
                DeviceTaskResponse::FileProgress { .. }
            ));
        }
        let expected = format!("{:x}", Sha256::digest(&bytes));
        assert!(
            matches!(stream.receive_json::<DeviceTaskResponse>(TIMEOUT).await.unwrap(),DeviceTaskResponse::FileComplete { size:received,sha256 } if received == size as u64 && sha256 == expected)
        );
        worker.await.unwrap().unwrap();
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), bytes);
        let snapshot = service.store.get_transfer(actor(), id).await.unwrap();
        assert_eq!(snapshot.state, "completed");
        assert_eq!(snapshot.published, Some(true));
    }
    first.close().await;
    second.close().await;
}

#[tokio::test]
async fn changed_source_bytes_fail_checksum_without_publishing() {
    let directory = tempfile::tempdir().unwrap();
    let service = service(directory.path()).await;
    let (first, second, client, server) = pair().await;
    let destination = directory.path().join("result.bin");
    tokio::fs::write(&destination, b"previous").await.unwrap();
    let original = b"original";
    let changed = b"modified";
    let id = RequestId::new();
    let worker = receiver(&service, &server).await;
    let mut stream = upload(&client, id, &destination, original).await;
    stream.send_binary_frame(changed, TIMEOUT).await.unwrap();
    assert!(matches!(
        stream
            .receive_json::<DeviceTaskResponse>(TIMEOUT)
            .await
            .unwrap(),
        DeviceTaskResponse::FileProgress { .. }
    ));
    assert!(matches!(
        stream
            .receive_json::<DeviceTaskResponse>(TIMEOUT)
            .await
            .unwrap(),
        DeviceTaskResponse::Error { .. }
    ));
    assert!(worker.await.unwrap().is_err());
    let snapshot = service.store.get_transfer(actor(), id).await.unwrap();
    assert_eq!(snapshot.offset, snapshot.size);
    assert_eq!(snapshot.state, "failed");
    assert_eq!(snapshot.published, Some(false));
    assert_eq!(tokio::fs::read(destination).await.unwrap(), b"previous");
    first.close().await;
    second.close().await;
}
