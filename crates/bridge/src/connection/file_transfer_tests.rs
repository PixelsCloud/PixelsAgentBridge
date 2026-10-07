use super::*;
use pab_protocol::*;
use pab_transport::{PabConnection, PabEndpoint, PabEndpointAddress, PabEndpointConfig};
const TIMEOUT: Duration = Duration::from_secs(5);

fn context() -> ExecutionContext {
    serde_json::from_value(serde_json::json!({"os_family":"linux","os_name":"Linux","os_version":"test","architecture":"x86_64","execution_scope":"native","path_style":"posix","interpreter":null,"cwd":"/home/fixture","environment_revision":"user-v1:test","identity":{"mode":"user","account_id":"uid:23001","account_name":"fixture","home":"/home/fixture","primary_group":23001,"session_id":null,"logon_id":null,"environment_source":"native_account"}})).unwrap()
}

pub(crate) async fn pair() -> (
    PabEndpoint,
    PabEndpoint,
    AuthenticatedDeviceConnection,
    PabConnection,
) {
    let config = PabEndpointConfig::new(vec!["https://127.0.0.1:1".parse().unwrap()]).unwrap();
    let key = || {
        iroh_base::SecretKey::from_bytes(
            &Sha256::digest(RequestId::new().to_string().as_bytes()).into(),
        )
    };
    let a = PabEndpoint::bind(config.clone(), key()).await.unwrap();
    let b = PabEndpoint::bind(config, key()).await.unwrap();
    let mut watch = b.watch_address();
    tokio::time::timeout(TIMEOUT, async {
        while watch.borrow().direct_addresses.is_empty() {
            watch.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let direct = watch
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
    let server = b.clone();
    let accept = tokio::spawn(async move { server.accept().await.unwrap().unwrap() });
    let connection = a
        .connect(
            *b.id().as_bytes(),
            &PabEndpointAddress {
                relay_urls: vec![],
                direct_addresses: direct,
            },
            TIMEOUT,
        )
        .await
        .unwrap();
    let server = accept.await.unwrap();
    let client = AuthenticatedDeviceConnection {
        connection,
        device_ref: DeviceRef {
            tenant_id: TenantId::from_u128(1),
            device_id: DeviceId::from_u128(2),
        },
        operator: OperatorRef::account(UserId::from_u128(3), EndpointKey::new([3; 32])),
        password_version: 1,
        authenticated_at_unix_ms: 1,
        operation_timeout: TIMEOUT,
    };
    (a, b, client, server)
}

#[tokio::test]
async fn transfer_v2_bridge_checks_identity_before_binary_and_resumes_download() {
    // A real QUIC peer exercises the Bridge wire client. Native user execution
    // is independently verified by the Executor's three-platform fixture.
    for scenario in [
        "upload",
        "service_upload",
        "resume_download",
        "wrong_identity",
        "old_peer",
        "rejected",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("中文 payload.bin");
        let bytes = b"binary\0payload with checksum".to_vec();
        let original = RequestId::new();
        let id = RequestId::new();
        let mut options = FileTransferOptions {
            execution: ExecutionSelection::User {
                context_ref: ExecutionContextRef::new(),
            },
            resume_from: None,
        };
        let mut selected_context = context();
        if scenario == "service_upload" {
            options.execution = Default::default();
            let identity = selected_context.identity.as_mut().unwrap();
            identity.mode = ExecutionMode::Service;
            identity.environment_source = ExecutionEnvironmentSource::ServiceProcess;
        }
        if scenario == "resume_download" {
            options.resume_from = Some(original);
            fs::write(partial_path(&path, original), &bytes[..7])
                .await
                .unwrap();
        } else {
            fs::write(&path, &bytes).await.unwrap();
        }
        let (a, b, client, server) = pair().await;
        let device = client.device_ref;
        let actor = client.operator;
        let expected_options = options.clone();
        let expected = bytes.clone();
        let observed_context = selected_context.clone();
        let job = tokio::spawn(async move {
            let mut env = server.accept_bi(TIMEOUT).await.unwrap();
            assert!(matches!(
                env.receive_json::<DeviceTaskRequest>(TIMEOUT)
                    .await
                    .unwrap(),
                DeviceTaskRequest::GetEnvironment { .. }
            ));
            let value = serde_json::json!({"type":"environment","transfer_schema_version":if scenario=="old_peer" {1}else{2},"context":{"device_ref":device,"execution":context(),"source":"executor_verified","observed_at_unix_ms":1,"freshness":"current"}});
            env.send_json(
                &serde_json::from_value::<DeviceTaskResponse>(value).unwrap(),
                TIMEOUT,
            )
            .await
            .unwrap();
            if scenario == "old_peer" {
                assert!(server.accept_bi(Duration::from_millis(200)).await.is_err());
                return;
            }
            let mut stream = server.accept_bi(TIMEOUT).await.unwrap();
            let DeviceTaskRequest::TransferFile { request, .. } = stream
                .receive_json::<DeviceTaskRequest>(TIMEOUT)
                .await
                .unwrap()
            else {
                panic!("user transfer fell back to legacy");
            };
            assert_eq!(request.execution, expected_options.execution);
            assert_eq!(request.resume_from, expected_options.resume_from);
            let mut observed = observed_context;
            if scenario == "wrong_identity" {
                observed.identity.as_mut().unwrap().account_id = "uid:23002".into();
            }
            if scenario == "rejected" {
                stream
                    .send_json(
                        &DeviceTaskResponse::Error {
                            code: DeviceTaskErrorCode::InvalidRequest,
                            message: "context unavailable".into(),
                        },
                        TIMEOUT,
                    )
                    .await
                    .unwrap();
                return;
            }
            let digest = format!("{:x}", Sha256::digest(&expected));
            let snapshot = TransferSnapshot {
                request_id: id,
                initiated_by: actor,
                direction: request.operation.direction().into(),
                path: request.operation.path().into(),
                state: "running".into(),
                offset: 0,
                size: expected.len() as u64,
                sha256: Some(digest.clone()),
                finished_at_unix_ms: None,
                message: None,
                published: None,
                execution_context: Some(observed),
            };
            stream
                .send_frame_json(&DeviceTaskResponse::TransferAccepted { snapshot }, TIMEOUT)
                .await
                .unwrap();
            if scenario == "wrong_identity" {
                assert!(
                    stream
                        .receive_binary_frame(Duration::from_millis(300))
                        .await
                        .is_err()
                );
                return;
            }
            let offset = if scenario == "resume_download" { 7 } else { 0 };
            stream
                .send_frame_json(
                    &DeviceTaskResponse::FileReady {
                        size: expected.len() as u64,
                        offset,
                        sha256: digest.clone(),
                    },
                    TIMEOUT,
                )
                .await
                .unwrap();
            if scenario == "resume_download" {
                assert!(
                    matches!(request.operation,FileTransferOperation::Download {offset:7,expected_sha256:Some(ref hash),..} if hash==&digest)
                );
                stream
                    .send_binary_frame(&expected[7..], TIMEOUT)
                    .await
                    .unwrap();
            } else {
                assert_eq!(
                    stream.receive_binary_frame(TIMEOUT).await.unwrap(),
                    expected
                );
                stream
                    .send_frame_json(
                        &DeviceTaskResponse::FileProgress {
                            offset: expected.len() as u64,
                        },
                        TIMEOUT,
                    )
                    .await
                    .unwrap();
            }
            stream
                .send_json(
                    &DeviceTaskResponse::FileComplete {
                        size: expected.len() as u64,
                        sha256: digest,
                    },
                    TIMEOUT,
                )
                .await
                .unwrap();
        });
        let control = TransferControl::default();
        control.expect_context(selected_context.clone());
        let result = if scenario == "resume_download" {
            client
                .download_file_with_options(
                    id,
                    "/remote/file",
                    &path,
                    false,
                    &options,
                    Some(&format!("{:x}", Sha256::digest(&bytes))),
                    &control,
                    |_, _| {},
                )
                .await
        } else {
            client
                .upload_file_with_options(
                    id,
                    &path,
                    "/remote/file",
                    false,
                    &options,
                    &control,
                    |_, _| {},
                )
                .await
        };
        assert_eq!(
            result.is_ok(),
            matches!(scenario, "upload" | "service_upload" | "resume_download"),
            "{scenario}: {result:?}"
        );
        if result.is_ok() {
            assert_eq!(
                control.accepted().unwrap().execution_context,
                Some(selected_context)
            );
        }
        if scenario == "resume_download" {
            assert_eq!(fs::read(&path).await.unwrap(), bytes);
            assert!(!partial_path(&path, original).exists());
            assert!(!partial_path(&path, id).exists());
        }
        if matches!(scenario, "old_peer" | "rejected") {
            assert!(!control.remote_requested());
        }
        job.await.unwrap();
        a.close().await;
        b.close().await;
    }
}
