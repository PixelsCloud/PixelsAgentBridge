//! Source integration only; public identity selection/installed MCP remain separate.
use super::transfer_tests::{actor, service};
use super::*;
use crate::task_service::transfer_engine::{LocalRecorder, TransferStream};
use crate::user_worker::transfer::Operation;
use pab_os_control::execution::PreparedUser;
use pab_protocol::*;
use std::{collections::VecDeque, io, path::PathBuf};

fn prepared(user: u32) -> PreparedUser {
    #[cfg(windows)]
    {
        PreparedUser::for_session(user).unwrap()
    }
    #[cfg(unix)]
    {
        PreparedUser::for_uid(user).unwrap()
    }
}
struct Workspace {
    path: PathBuf,
    home: PathBuf,
}
impl Drop for Workspace {
    fn drop(&mut self) {
        if !self
            .path
            .file_name()
            .is_some_and(|v| v.to_string_lossy().starts_with("pab-transfer-acceptance-"))
        {
            return;
        }
        if let (Ok(path), Ok(home)) = (self.path.canonicalize(), self.home.canonicalize()) {
            if path.parent() == Some(home.as_path()) && !self.path.is_symlink() {
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }
}
#[derive(Default)]
struct TestStream {
    input: VecDeque<Vec<u8>>,
    output: Vec<u8>,
    responses: Vec<DeviceTaskResponse>,
    resume_from: Option<RequestId>,
}
impl TransferStream for TestStream {
    async fn response(
        &mut self,
        value: &DeviceTaskResponse,
        _: bool,
        _: Duration,
    ) -> Result<(), TaskServiceError> {
        self.responses.push(value.clone());
        Ok(())
    }
    async fn receive_binary_frame(&mut self, _: Duration) -> Result<Vec<u8>, TaskServiceError> {
        self.input
            .pop_front()
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "test disconnect").into())
    }
    async fn send_binary_frame(
        &mut self,
        bytes: &[u8],
        _: Duration,
    ) -> Result<(), TaskServiceError> {
        assert!(!bytes.is_empty() && bytes.len() <= 64 * 1024);
        self.output.extend_from_slice(bytes);
        Ok(())
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn execute<'a>(
    svc: &'a TaskService,
    user: u32,
    operation: Operation,
    stream: &'a mut TestStream,
    cancelled: bool,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = (RequestId, io::Result<()>)> + 'a>> {
    Box::pin(execute_inner(svc, user, operation, stream, cancelled))
}
async fn execute_inner(
    svc: &TaskService,
    user: u32,
    operation: Operation,
    stream: &mut TestStream,
    cancelled: bool,
) -> (RequestId, io::Result<()>) {
    let id = RequestId::new();
    if !cancelled {
        let identity = prepared(user)
            .identity()
            .observation(
                ExecutionMode::User,
                ExecutionEnvironmentSource::NativeAccount,
            )
            .unwrap();
        let context_ref = svc
            .ui_connection
            .execution_contexts
            .lock()
            .await
            .register(
                pab_task_runtime::ExecutionCaller {
                    device: svc.device_ref,
                    actor: actor(),
                    connection: svc.ui_connection.id,
                },
                identity.clone(),
            )
            .unwrap();
        let request = FileTransferRequest {
            request_id: id,
            execution: ExecutionSelection::User { context_ref },
            resume_from: stream.resume_from,
            operation: match operation {
                Operation::Upload {
                    path,
                    size,
                    sha256,
                    overwrite,
                } => FileTransferOperation::Upload {
                    path,
                    size,
                    sha256,
                    overwrite,
                },
                Operation::Download {
                    path,
                    offset,
                    expected_sha256,
                } => FileTransferOperation::Download {
                    path,
                    offset,
                    expected_sha256,
                },
                Operation::Reconcile { .. } => panic!("separate reconciliation"),
            },
        };
        let result = dispatch(svc, &request, stream).await;
        if let Ok(snapshot) = svc.store.get_transfer(actor(), id).await {
            assert_eq!(
                snapshot
                    .execution_context
                    .as_ref()
                    .unwrap()
                    .identity
                    .as_ref(),
                Some(&identity)
            );
            let fresh = svc.for_ui_connection();
            let mut repeated = TestStream::default();
            dispatch(&fresh, &request, &mut repeated).await.unwrap();
            assert!(
                matches!(repeated.responses.first(),Some(DeviceTaskResponse::Transfer { snapshot: observed }) if observed.execution_context==snapshot.execution_context)
            );
            let mut changed = request.clone();
            changed.execution = Default::default();
            assert!(
                dispatch(svc, &changed, &mut TestStream::default())
                    .await
                    .is_err()
            );
        }
        return (id, result);
    }
    match &operation {
        Operation::Upload {
            path, size, sha256, ..
        } => svc
            .store
            .start_transfer(id, actor(), "receive", path, *size, Some(sha256))
            .await
            .unwrap(),
        Operation::Download { path, .. } => svc
            .store
            .start_transfer(id, actor(), "send", path, 0, None)
            .await
            .unwrap(),
        Operation::Reconcile { .. } => panic!("use the read-only reconciliation entry"),
    }
    let (_cancel, receive) = watch::channel(cancelled);
    let result = crate::user_worker::transfer::execute(
        &PathBuf::from(std::env::var_os("PAB_EXECUTION_TEST_WORKER").unwrap()),
        prepared(user),
        operation,
        LocalRecorder {
            store: svc.store.clone(),
            request_id: id,
        },
        svc.file_coordinator(),
        stream,
        receive,
    )
    .await;
    if let Err(error) = &result {
        svc.store
            .finish_transfer(id, "failed", Some(&error.to_string()))
            .await
            .unwrap();
    }
    (id, result)
}
async fn dispatch(
    svc: &TaskService,
    request: &FileTransferRequest,
    fixture: &mut TestStream,
) -> io::Result<()> {
    let (a, b, client, server) = super::transfer_tests::pair().await;
    let service = svc.clone();
    let timeout = Duration::from_secs(15);
    let work = tokio::spawn(async move {
        service
            .handle_stream(actor(), server.accept_bi(timeout).await.unwrap(), timeout)
            .await
    });
    let mut stream = client.open_bi(timeout).await.unwrap();
    stream
        .send_frame_json(
            &DeviceTaskRequest::TransferFile {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request: request.clone(),
            },
            timeout,
        )
        .await
        .unwrap();
    let result = async {
        loop {
            let response: DeviceTaskResponse = stream
                .receive_json(timeout)
                .await
                .map_err(io::Error::other)?;
            fixture.responses.push(response.clone());
            match response {
                DeviceTaskResponse::Transfer { .. } | DeviceTaskResponse::FileComplete { .. } => {
                    return Ok(());
                }
                DeviceTaskResponse::Error { message, .. } => return Err(io::Error::other(message)),
                DeviceTaskResponse::FileReady { size, offset, .. }
                    if matches!(request.operation, FileTransferOperation::Download { .. }) =>
                {
                    let mut received = offset;
                    while received < size {
                        let bytes = stream
                            .receive_binary_frame(timeout)
                            .await
                            .map_err(io::Error::other)?;
                        received += bytes.len() as u64;
                        assert!(received <= size);
                        fixture.output.extend(bytes);
                    }
                }
                DeviceTaskResponse::FileReady { size, offset, .. } => {
                    if offset < size {
                        let bytes = fixture
                            .input
                            .pop_front()
                            .ok_or_else(|| io::Error::other("fixture disconnect"))?;
                        stream
                            .send_binary_frame(&bytes, timeout)
                            .await
                            .map_err(io::Error::other)?;
                    }
                }
                DeviceTaskResponse::FileProgress { offset } => {
                    let FileTransferOperation::Upload { size, .. } = &request.operation else {
                        panic!("download progress frame")
                    };
                    if offset < *size {
                        let bytes = fixture
                            .input
                            .pop_front()
                            .ok_or_else(|| io::Error::other("fixture disconnect"))?;
                        stream
                            .send_binary_frame(&bytes, timeout)
                            .await
                            .map_err(io::Error::other)?;
                    }
                }
                _ => panic!("unexpected transfer response"),
            }
        }
    }
    .await;
    drop(stream);
    let _ = tokio::time::timeout(Duration::from_secs(20), work)
        .await
        .unwrap()
        .unwrap();
    a.close().await;
    b.close().await;
    result
}

fn upload(path: &Path, bytes: &[u8], overwrite: bool) -> Operation {
    Operation::Upload {
        path: path.to_str().unwrap().into(),
        size: bytes.len() as u64,
        sha256: digest(bytes),
        overwrite,
    }
}
fn input(bytes: &[u8]) -> TestStream {
    TestStream {
        input: bytes.chunks(65536).map(Vec::from).collect(),
        ..Default::default()
    }
}

#[tokio::test]
#[ignore = "requires privileged native service and PAB_EXECUTION_TEST_WORKER/PAB_EXECUTION_TEST_USER"]
async fn native_user_transfer_worker_acceptance() {
    let user = std::env::var("PAB_EXECUTION_TEST_USER")
        .unwrap()
        .parse()
        .unwrap();
    let identity = prepared(user).identity().clone();
    let database = tempfile::tempdir().unwrap();
    let mut svc = service(database.path()).await;
    svc.worker_executable = Some(
        std::env::var_os("PAB_EXECUTION_TEST_WORKER")
            .unwrap()
            .into(),
    );
    let root = Workspace {
        path: identity
            .home
            .join(format!("pab-transfer-acceptance-{}", RequestId::new())),
        home: identity.home.clone(),
    };
    let request = FileSystemRequest {
        execution: Default::default(),
        request_id: RequestId::new(),
        path: root.path.to_str().unwrap().into(),
        operation: FileSystemAction::Mkdir {
            parents: false,
            exist_ok: false,
        },
        payload_size: 0,
        payload_sha256: None,
    };
    let mut context = svc.execution_context.clone();
    context.identity = Some(
        identity
            .observation(
                ExecutionMode::User,
                ExecutionEnvironmentSource::NativeAccount,
            )
            .unwrap(),
    );
    let (_send, receive) = watch::channel(false);
    let created = crate::user_worker::filesystem::execute(
        &PathBuf::from(std::env::var_os("PAB_EXECUTION_TEST_WORKER").unwrap()),
        prepared(user),
        request,
        vec![],
        context,
        svc.file_coordinator(),
        receive,
    )
    .await
    .unwrap();
    assert_eq!(created.0.state, "completed", "{:?}", created.0);
    let path = root.path.join("中文 payload.bin");
    let bytes: Vec<u8> = (0..196607).map(|n| (n % 251) as u8).collect();
    let mut interrupted = TestStream {
        input: [bytes[..65536].to_vec()].into(),
        ..Default::default()
    };
    let (id, result) = execute(
        &svc,
        user,
        upload(&path, &bytes, false),
        &mut interrupted,
        false,
    )
    .await;
    assert!(result.is_err());
    assert!(!path.exists());
    assert_eq!(
        svc.store.get_transfer(actor(), id).await.unwrap().offset,
        65536
    );
    // New attempt resumes only matching hash bytes, in the same native account.
    let mut changed_user = svc.store.transfer_request(actor(), id).await.unwrap();
    changed_user.request_id = RequestId::new();
    changed_user.resume_from = Some(id);
    changed_user.execution = Default::default();
    assert!(
        dispatch(&svc, &changed_user, &mut input(&bytes))
            .await
            .is_err()
    );
    assert!(matches!(
        svc.store
            .get_transfer(actor(), changed_user.request_id)
            .await,
        Err(TaskStoreError::NotFound)
    ));
    let mut changed_content = input(b"different");
    changed_content.resume_from = Some(id);
    let (rejected_id, result) = execute(
        &svc,
        user,
        upload(&path, b"different", false),
        &mut changed_content,
        false,
    )
    .await;
    assert!(result.is_err());
    assert!(matches!(
        svc.store.get_transfer(actor(), rejected_id).await,
        Err(TaskStoreError::NotFound)
    ));
    assert!(!path.exists());
    let mut resumed = input(&bytes[65536..]);
    resumed.resume_from = Some(id);
    let (id, result) = execute(
        &svc,
        user,
        upload(&path, &bytes, false),
        &mut resumed,
        false,
    )
    .await;
    result.unwrap();
    let transfer_context = svc
        .store
        .get_transfer(actor(), id)
        .await
        .unwrap()
        .execution_context
        .unwrap();
    assert!(matches!(
        resumed.responses.first(),
        Some(DeviceTaskResponse::FileReady { offset: 65536, .. })
    ));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(
        svc.store.get_transfer(actor(), id).await.unwrap().published,
        Some(true)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(&path).unwrap().uid(), user);
    }
    #[cfg(windows)]
    {
        let output=std::process::Command::new("powershell.exe").args(["-NoProfile","-NonInteractive","-Command",&format!("(Get-Acl -LiteralPath '{}').GetOwner([Security.Principal.SecurityIdentifier]).Value",path.to_str().unwrap().replace('\'',"''"))]).output().unwrap();
        assert!(output.status.success());
        let owner = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert!(
            owner == identity.account_id || owner == "S-1-5-32-544",
            "{owner}"
        );
        assert_ne!(owner, "S-1-5-18");
    }
    let mut downloaded = TestStream::default();
    let (id, result) = execute(
        &svc,
        user,
        Operation::Download {
            path: path.to_str().unwrap().into(),
            offset: 65536,
            expected_sha256: None,
        },
        &mut downloaded,
        false,
    )
    .await;
    result.unwrap();
    assert_eq!(downloaded.output, bytes[65536..]);
    assert_eq!(
        svc.store.get_transfer(actor(), id).await.unwrap().sha256,
        Some(digest(&bytes))
    );
    let mut resumed_download = TestStream {
        resume_from: Some(id),
        ..Default::default()
    };
    execute(
        &svc,
        user,
        Operation::Download {
            path: path.to_str().unwrap().into(),
            offset: 131072,
            expected_sha256: Some(digest(&bytes)),
        },
        &mut resumed_download,
        false,
    )
    .await
    .1
    .unwrap();
    assert_eq!(resumed_download.output, bytes[131072..]);
    let mut changed_download = TestStream::default();
    assert!(
        execute(
            &svc,
            user,
            Operation::Download {
                path: path.to_str().unwrap().into(),
                offset: 65536,
                expected_sha256: Some(digest(b"old content")),
            },
            &mut changed_download,
            false
        )
        .await
        .1
        .is_err()
    );
    assert!(changed_download.output.is_empty());
    let held = svc.upload_locks.try_acquire(&path).await.unwrap().unwrap();
    assert!(
        execute(
            &svc,
            user,
            upload(&path, b"blocked", true),
            &mut input(b"blocked"),
            false
        )
        .await
        .1
        .is_err()
    );
    drop(held);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let mut corrupt = input(b"bad");
    assert!(
        execute(&svc, user, upload(&path, b"new", true), &mut corrupt, false)
            .await
            .1
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(
        execute(
            &svc,
            user,
            upload(&path, b"cancelled", true),
            &mut input(b"cancelled"),
            true
        )
        .await
        .1
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let empty = root.path.join("empty.bin");
    execute(
        &svc,
        user,
        upload(&empty, b"", false),
        &mut TestStream::default(),
        false,
    )
    .await
    .1
    .unwrap();
    assert_eq!(std::fs::metadata(empty).unwrap().len(), 0);
    let private = database.path().join("private");
    std::fs::create_dir(&private).unwrap();
    std::fs::write(private.join("secret.bin"), b"private").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[cfg(windows)]
    {
        let output = std::process::Command::new("icacls.exe")
            .arg(&private)
            .args([
                "/inheritance:r",
                "/grant:r",
                "*S-1-5-18:(OI)(CI)F",
                "/deny",
                &format!("*{}:(OI)(CI)F", identity.account_id),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    assert!(
        execute(
            &svc,
            user,
            upload(&private.join("denied.bin"), b"no", false),
            &mut input(b"no"),
            false
        )
        .await
        .1
        .is_err()
    );
    let mut denied = TestStream::default();
    assert!(
        execute(
            &svc,
            user,
            Operation::Download {
                path: private.join("secret.bin").to_str().unwrap().into(),
                offset: 0,
                expected_sha256: None
            },
            &mut denied,
            false
        )
        .await
        .1
        .is_err()
    );
    assert!(denied.output.is_empty());
    assert!(!private.join("denied.bin").exists());
    // Simulate publication before its durable completion receipt. Observation
    // must use the frozen native account, even after the connection changes.
    for (target, content, expected_state) in [
        (&path, bytes.as_slice(), "completed"),
        (
            &private.join("secret.bin"),
            b"private".as_slice(),
            "committing",
        ),
    ] {
        let req = FileTransferRequest {
            request_id: RequestId::new(),
            execution: ExecutionSelection::User {
                context_ref: ExecutionContextRef::new(),
            },
            resume_from: None,
            operation: FileTransferOperation::Upload {
                path: target.to_str().unwrap().into(),
                size: content.len() as u64,
                sha256: digest(content),
                overwrite: true,
            },
        };
        svc.store
            .accept_transfer(actor(), &req, &transfer_context)
            .await
            .unwrap();
        svc.store
            .transfer_progress(req.request_id, content.len() as u64, content.len() as u64)
            .await
            .unwrap();
        svc.store
            .begin_transfer_publication(req.request_id)
            .await
            .unwrap();
        assert_eq!(
            svc.store
                .get_transfer(actor(), req.request_id)
                .await
                .unwrap()
                .state,
            "committing"
        );
        let observed = svc
            .for_ui_connection()
            .lookup_transfer(actor(), req.request_id)
            .await
            .unwrap();
        assert_eq!(observed.state, expected_state, "{observed:?}");
        assert_eq!(observed.execution_context, Some(transfer_context.clone()));
    }
    assert!(svc.upload_locks.try_acquire(&path).await.unwrap().is_some());
}

#[tokio::test]
async fn invalid_transfer_identity_rejects_before_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let request = FileTransferRequest {
        request_id: RequestId::new(),
        execution: ExecutionSelection::User {
            context_ref: ExecutionContextRef::new(),
        },
        resume_from: None,
        operation: FileTransferOperation::Upload {
            path: dir.path().join("never.bin").to_str().unwrap().into(),
            size: 1,
            sha256: digest(b"x"),
            overwrite: false,
        },
    };
    assert!(dispatch(&svc, &request, &mut input(b"x")).await.is_err());
    assert!(matches!(
        svc.store.get_transfer(actor(), request.request_id).await,
        Err(TaskStoreError::NotFound)
    ));
    assert!(!dir.path().join("never.bin").exists());
}
