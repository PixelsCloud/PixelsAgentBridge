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
async fn execute(
    svc: &TaskService,
    user: u32,
    operation: Operation,
    stream: &mut TestStream,
    cancelled: bool,
) -> (RequestId, io::Result<()>) {
    let id = RequestId::new();
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
    let svc = service(database.path()).await;
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
    let mut resumed = input(&bytes[65536..]);
    let (id, result) = execute(
        &svc,
        user,
        upload(&path, &bytes, false),
        &mut resumed,
        false,
    )
    .await;
    result.unwrap();
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
                offset: 0
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
    assert!(svc.upload_locks.try_acquire(&path).await.unwrap().is_some());
}
