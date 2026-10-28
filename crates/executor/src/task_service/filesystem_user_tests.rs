//! Native executor integration; installed MCP host acceptance remains separate.
use super::transfer_tests::{actor, service};
use super::*;
use pab_os_control::execution::PreparedUser;
use pab_protocol::*;
use std::path::PathBuf;

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
            .is_some_and(|s| s.to_string_lossy().starts_with("pab-file-acceptance-"))
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
fn run<'a>(
    svc: &'a TaskService,
    user: u32,
    path: &'a Path,
    action: FileSystemAction,
    payload: Vec<u8>,
    cancel: bool,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = (FileSystemReply, Vec<u8>)> + 'a>> {
    // Keep each native operation out of the large workflow future's stack frame.
    Box::pin(run_inner(svc, user, path, action, payload, cancel))
}

async fn run_inner(
    svc: &TaskService,
    user: u32,
    path: &Path,
    action: FileSystemAction,
    payload: Vec<u8>,
    cancel: bool,
) -> (FileSystemReply, Vec<u8>) {
    let mut request = FileSystemRequest {
        execution: Default::default(),
        request_id: RequestId::new(),
        path: path.to_str().unwrap().into(),
        payload_size: payload.len() as u32,
        payload_sha256: action
            .has_payload()
            .then(|| filesystem_io::digest(&payload)),
        operation: action,
    };
    if !cancel {
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
        request.execution = ExecutionSelection::User { context_ref };
        let mut result = dispatch(svc, &request, &payload).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while matches!(result.0.state.as_str(), "running" | "cancel_requested") {
            assert!(tokio::time::Instant::now() < deadline, "{:?}", result.0);
            tokio::time::sleep(Duration::from_millis(15)).await;
            result.0 = svc
                .lookup_filesystem(actor(), request.request_id)
                .await
                .unwrap();
        }
        assert_eq!(
            result
                .0
                .execution_context
                .as_ref()
                .unwrap()
                .identity
                .as_ref(),
            Some(&identity)
        );
        // A new connection cannot use this selection, but can observe/replay an
        // already accepted request without changing its original user identity.
        let fresh = svc.for_ui_connection();
        let historical = fresh
            .execute_filesystem(actor(), &request, &payload)
            .await
            .unwrap()
            .0;
        assert_eq!(historical.execution_context, result.0.execution_context);
        if result.0.kind == "file_read" && result.0.state == "completed" {
            assert_eq!(historical.error.unwrap().code, "read_request_consumed");
        } else {
            assert_eq!(historical.state, result.0.state);
        }
        let mut changed = request.clone();
        changed.execution = Default::default();
        assert!(matches!(
            svc.execute_filesystem(actor(), &changed, &payload).await,
            Err(TaskServiceError::Store(TaskStoreError::RequestConflict))
        ));
        return result;
    }
    svc.store
        .accept_filesystem(
            actor(),
            &request,
            &filesystem_io::digest(&serde_json::to_vec(&request).unwrap()),
        )
        .await
        .unwrap();
    let (send, receive) = watch::channel(cancel);
    let mut context = svc.execution_context.clone();
    context.identity = Some(
        prepared(user)
            .identity()
            .observation(
                ExecutionMode::User,
                ExecutionEnvironmentSource::NativeAccount,
            )
            .unwrap(),
    );
    let result = crate::user_worker::filesystem::execute(
        &PathBuf::from(std::env::var_os("PAB_EXECUTION_TEST_WORKER").unwrap()),
        prepared(user),
        request.clone(),
        payload,
        context,
        svc.file_coordinator(),
        receive,
    )
    .await
    .unwrap();
    drop(send);
    svc.store.finish_filesystem(&result.0).await.unwrap();
    let saved = svc
        .store
        .get_filesystem(actor(), request.request_id)
        .await
        .unwrap();
    assert_eq!(saved.state, result.0.state);
    result
}

async fn dispatch(
    svc: &TaskService,
    request: &FileSystemRequest,
    payload: &[u8],
) -> (FileSystemReply, Vec<u8>) {
    if !matches!(
        request.operation,
        FileSystemAction::Write { .. }
            | FileSystemAction::Read { .. }
            | FileSystemAction::ListDirectory { .. }
    ) {
        return svc
            .execute_filesystem(actor(), request, payload)
            .await
            .unwrap();
    }
    let timeout = Duration::from_secs(15);
    let (a, b, client, server) = super::transfer_tests::pair().await;
    let worker = svc.clone();
    let task = tokio::spawn(async move {
        worker
            .handle_stream(actor(), server.accept_bi(timeout).await.unwrap(), timeout)
            .await
            .unwrap();
    });
    let mut stream = client.open_bi(timeout).await.unwrap();
    stream
        .send_frame_json(
            &DeviceTaskRequest::FileSystem {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request: request.clone(),
            },
            timeout,
        )
        .await
        .unwrap();
    for bytes in payload.chunks(64 * 1024) {
        stream.send_binary_frame(bytes, timeout).await.unwrap();
    }
    stream.finish_send(timeout).await.unwrap();
    let response: DeviceTaskResponse = stream.receive_json(timeout).await.unwrap();
    let DeviceTaskResponse::FileSystem { reply } = response else {
        panic!("{response:?}")
    };
    let mut bytes = Vec::new();
    while bytes.len() < reply.data_size as usize {
        bytes.extend(stream.receive_binary_frame(timeout).await.unwrap());
    }
    stream.expect_receive_end(timeout).await.unwrap();
    assert_eq!(bytes.len(), reply.data_size as usize);
    if let Some(hash) = &reply.data_sha256 {
        assert_eq!(*hash, filesystem_io::digest(&bytes));
    }
    task.await.unwrap();
    a.close().await;
    b.close().await;
    (*reply, bytes)
}
async fn success(
    svc: &TaskService,
    user: u32,
    path: &Path,
    action: FileSystemAction,
    payload: Vec<u8>,
) -> (FileSystemReply, Vec<u8>) {
    let result = run(svc, user, path, action, payload, false).await;
    assert_eq!(result.0.state, "completed", "{:?}", result.0);
    result
}
fn write(overwrite: bool, hash: Option<String>) -> FileSystemAction {
    FileSystemAction::Write {
        encoding: TextEncoding::Utf8,
        overwrite,
        expected_hash: hash,
    }
}

#[tokio::test]
async fn invalid_user_file_context_is_rejected_before_acceptance() {
    let directory = tempfile::tempdir().unwrap();
    let svc = service(directory.path()).await;
    let request = FileSystemRequest {
        execution: ExecutionSelection::User {
            context_ref: ExecutionContextRef::new(),
        },
        request_id: RequestId::new(),
        path: directory
            .path()
            .join("untouched.txt")
            .to_str()
            .unwrap()
            .into(),
        operation: write(false, None),
        payload_size: 1,
        payload_sha256: Some(filesystem_io::digest(b"x")),
    };
    assert!(matches!(
        svc.execute_filesystem(actor(), &request, b"x").await,
        Err(TaskServiceError::ExecutionContext(_))
    ));
    assert!(
        svc.store
            .get_filesystem(actor(), request.request_id)
            .await
            .is_err()
    );
    assert!(!Path::new(&request.path).exists());
    let timeout = Duration::from_secs(5);
    let (a, b, client, server) = super::transfer_tests::pair().await;
    let worker = svc.clone();
    let task = tokio::spawn(async move {
        worker
            .handle_stream(actor(), server.accept_bi(timeout).await.unwrap(), timeout)
            .await
            .unwrap();
    });
    let mut stream = client.open_bi(timeout).await.unwrap();
    stream
        .send_frame_json(
            &DeviceTaskRequest::FileSystem {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request: request.clone(),
            },
            timeout,
        )
        .await
        .unwrap();
    stream.send_binary_frame(b"x", timeout).await.unwrap();
    stream.finish_send(timeout).await.unwrap();
    let response: DeviceTaskResponse = stream.receive_json(timeout).await.unwrap();
    assert!(
        matches!(response, DeviceTaskResponse::Error { .. }),
        "{response:?}"
    );
    task.await.unwrap();
    assert!(
        svc.store
            .get_filesystem(actor(), request.request_id)
            .await
            .is_err()
    );
    a.close().await;
    b.close().await;
}

#[tokio::test]
#[cfg(unix)]
#[ignore = "requires an explicitly prepared native user and matching PAB_EXECUTION_TEST_WORKER; owns only a generated home fixture"]
async fn native_user_directory_publication_recovery_acceptance() {
    // Keep the composed acceptance workflow off the default libtest stack.
    Box::pin(native_user_directory_publication_recovery_case()).await;
}

#[cfg(unix)]
async fn native_user_directory_publication_recovery_case() {
    use std::os::unix::fs::MetadataExt;
    let user: u32 = std::env::var("PAB_EXECUTION_TEST_USER").unwrap().parse().unwrap();
    let identity = prepared(user).identity().clone();
    let worker = PathBuf::from(std::env::var_os("PAB_EXECUTION_TEST_WORKER").unwrap());
    assert!(worker.is_file(), "a matching, runnable Executor worker is required");
    for fault in ["none", "result-commit", "release-notification"] {
        let database = tempfile::tempdir().unwrap();
        let database_root = database.path().to_owned();
        let mut svc = Box::pin(service(&database_root)).await.for_ui_connection();
        svc.worker_executable = Some(worker.clone());
        let root = Workspace {
            path: identity.home.join(format!("pab-file-acceptance-{}", RequestId::new())),
            home: identity.home.clone(),
        };
        eprintln!("PAB_FILE_TRANSFER_FIXTURE_ROOT={}", root.path.display());
        eprintln!("PAB_NATIVE_DIRECTORY_DATABASE_ROOT={}", database_root.display());
        success(&svc, user, &root.path, FileSystemAction::Mkdir {parents:false,exist_ok:false}, vec![]).await;
        assert_eq!(std::fs::metadata(&root.path).unwrap().uid(), user);
        let child=root.path.join("child");let native=pab_file_manager::encode_path(&child);
        let parent=pab_file_manager::DirectoryObject::open(&pab_file_manager::encode_path(&root.path),false).unwrap().identity().to_owned();
        let observed=prepared(user).identity().observation(ExecutionMode::User,ExecutionEnvironmentSource::NativeAccount).unwrap();
        let context_ref=svc.ui_connection.execution_contexts.lock().await.register(
            pab_task_runtime::ExecutionCaller {device:svc.device_ref,actor:actor(),connection:svc.ui_connection.id},observed.clone()).unwrap();
        let mut expected_context=svc.execution_context.clone();expected_context.identity=Some(observed.clone());
        let request=FileSystemRequest {request_id:RequestId::new(),execution:ExecutionSelection::User {context_ref},path:"file-manager".into(),operation:FileSystemAction::Managed {expected_execution:Some(FileExecutionIdentity::from_context(&expected_context).unwrap()),operation:FileManagerOperation::PrepareDirectory {path:native.clone(),expected_parent:parent,existing_object:None}},payload_size:0,payload_sha256:None};
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(database_root.join("executor.sqlite3"))).await.unwrap();
        let id=request.request_id.to_string();
        if fault=="result-commit" {
            // The real child must publish before the parent's final transaction
            // fails. Progress/admission remain available for original-ID lookup.
            sqlx::query(&format!("CREATE TRIGGER reject_native_directory_result BEFORE UPDATE OF reply_json ON filesystem_results WHEN NEW.request_id='{id}' AND json_extract(NEW.reply_json,'$.state')='completed' BEGIN SELECT RAISE(ABORT,'injected native directory commit failure'); END")).execute(&pool).await.unwrap();
        }else if fault=="release-notification" {
            sqlx::query(&format!("CREATE TRIGGER reject_native_directory_release BEFORE UPDATE OF creation_receipt_pending ON filesystem_execution WHEN NEW.request_id='{id}' AND NEW.creation_receipt_pending=0 BEGIN SELECT RAISE(ABORT,'injected native release notification failure'); END")).execute(&pool).await.unwrap();
        }
        let first=tokio::time::timeout(Duration::from_secs(45),Box::pin(svc.execute_filesystem(actor(),&request,&[]))).await.expect("native directory operation did not settle").unwrap().0;
        assert_eq!(first.state,if fault=="result-commit" {"unconfirmed"}else{"completed"},"{first:?}");
        assert_eq!(first.execution_context.as_ref().unwrap().identity.as_ref(),Some(&observed));
        assert_eq!(std::fs::metadata(&child).unwrap().uid(),user,"directory must be created by the selected native user");
        let original_identity=pab_file_manager::DirectoryObject::open(&native,false).unwrap().identity().to_owned();
        let admitted=svc.store.filesystem_execution(actor(),request.request_id).await.unwrap().unwrap();
        assert_eq!(admitted.request,request);assert_eq!(admitted.context,first.execution_context);
        assert_eq!(admitted.creation_receipt_pending,fault!="none");
        sqlx::query("DROP TRIGGER IF EXISTS reject_native_directory_result").execute(&pool).await.unwrap();
        sqlx::query("DROP TRIGGER IF EXISTS reject_native_directory_release").execute(&pool).await.unwrap();
        std::fs::rename(&child,root.path.join("original")).unwrap();std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("sentinel"),b"replacement must remain unchanged").unwrap();
        let fresh=svc.for_ui_connection();
        let final_reply=tokio::time::timeout(Duration::from_secs(30),Box::pin(fresh.lookup_filesystem(actor(),request.request_id))).await.expect("native directory recovery did not settle").unwrap();
        assert_eq!(final_reply.state,"completed","{final_reply:?}");assert!(final_reply.error.is_none());
        assert_eq!(final_reply.execution_context,first.execution_context);
        assert_eq!(final_reply.file_manager.as_ref().unwrap().result,FileManagerResult::DirectoryPrepared {path:native,identity:original_identity,created:true});
        assert!(!fresh.store.filesystem_execution(actor(),request.request_id).await.unwrap().unwrap().creation_receipt_pending);
        assert_eq!(Box::pin(fresh.execute_filesystem(actor(),&request,&[])).await.unwrap().0,final_reply,"replay must use the original identity despite the connection-bound user selection expiring");
        let mut changed=request.clone();changed.execution=ExecutionSelection::Service {};
        let rejected=Box::pin(fresh.execute_filesystem(actor(),&changed,&[])).await;
        assert!(matches!(rejected,Err(TaskServiceError::InvalidRequest("file-manager execution mode differs from the pinned identity"))),"{rejected:?}");
        // A syntactically valid service request must also be refused for the
        // original user-owned request ID, before any service mutation occurs.
        let FileSystemAction::Managed {expected_execution,..}=&mut changed.operation else {unreachable!()};
        *expected_execution=Some(FileExecutionIdentity::from_context(&fresh.execution_context).unwrap());
        changed.validate().unwrap();
        let rejected=Box::pin(fresh.execute_filesystem(actor(),&changed,&[])).await;
        assert!(matches!(rejected,Err(TaskServiceError::Store(TaskStoreError::RequestConflict))),"{rejected:?}");
        assert_eq!(fresh.store.filesystem_execution(actor(),request.request_id).await.unwrap().unwrap().request,request);
        assert_eq!(std::fs::read(child.join("sentinel")).unwrap(),b"replacement must remain unchanged");
        assert_eq!(std::fs::read_dir(root.path.join("original")).unwrap().count(),0);
        pool.close().await;svc.close_for_test().await;drop((fresh,svc));
        let home_root=root.path.clone();drop(root);assert!(!home_root.exists(),"owned native-user fixture cleanup failed");
        database.close().unwrap();assert!(!database_root.exists(),"owned task database fixture cleanup failed");
        eprintln!("native_user_directory_recovery phase={fault} uid={user} original_id={id} fixture_cleanup_verified=true");
    }
}

#[tokio::test]
#[ignore = "requires native user and PAB_EXECUTION_TEST_WORKER; owns only a generated home fixture"]
async fn native_user_filesystem_worker_acceptance() {
    let user: u32 = std::env::var("PAB_EXECUTION_TEST_USER")
        .unwrap()
        .parse()
        .unwrap();
    let identity = prepared(user).identity().clone();
    let database = tempfile::tempdir().unwrap();
    let mut svc = service(database.path()).await.for_ui_connection();
    svc.worker_executable = Some(
        std::env::var_os("PAB_EXECUTION_TEST_WORKER")
            .unwrap()
            .into(),
    );
    let root = Workspace {
        path: identity
            .home
            .join(format!("pab-file-acceptance-{}", RequestId::new())),
        home: identity.home.clone(),
    };
    success(
        &svc,
        user,
        &root.path,
        FileSystemAction::Mkdir {
            parents: false,
            exist_ok: false,
        },
        vec![],
    )
    .await;
    let file = root.path.join("中文 data.txt");
    let text = format!("ORIGINAL\n{}", "中文 line\n".repeat(8000)); // >64 KiB crosses binary frames.
    let written = success(
        &svc,
        user,
        &file,
        write(false, None),
        text.as_bytes().to_vec(),
    )
    .await
    .0;
    assert_eq!(
        written.metadata.as_ref().unwrap().sha256,
        Some(filesystem_io::digest(text.as_bytes()))
    );
    assert_eq!(std::fs::read(&file).unwrap(), text.as_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(&file).unwrap().uid(), user);
        assert_eq!(std::fs::metadata(&root.path).unwrap().uid(), user);
    }
    #[cfg(windows)]
    {
        // The default owner of an administrator token may be Administrators;
        // normal-user files must carry their SID, never the SYSTEM service SID.
        let output = std::process::Command::new("powershell.exe").args([
            "-NoProfile", "-NonInteractive", "-Command",
            &format!("(Get-Acl -LiteralPath '{}').GetOwner([Security.Principal.SecurityIdentifier]).Value", file.to_str().unwrap().replace('\'', "''"))
        ]).output().unwrap();
        assert!(output.status.success());
        let owner = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert!(
            owner == identity.account_id || owner == "S-1-5-32-544",
            "{owner}"
        );
        assert_ne!(owner, "S-1-5-18");
    }
    let read = success(
        &svc,
        user,
        &file,
        FileSystemAction::Read {
            range: TextReadRange::Bytes {
                offset: 0,
                max_bytes: 16000,
            },
            encoding: None,
            expected_hash: written.metadata.as_ref().unwrap().sha256.clone(),
        },
        vec![],
    )
    .await;
    assert!(read.1.starts_with(b"ORIGINAL\n"));
    assert!(std::str::from_utf8(&read.1).unwrap().contains("中文"));
    let stat = success(
        &svc,
        user,
        &file,
        FileSystemAction::Stat {
            follow_symlinks: false,
        },
        vec![],
    )
    .await
    .0;
    assert_eq!(stat.metadata.unwrap().size, text.len() as u64);
    let hash = success(&svc, user, &file, FileSystemAction::Hash, vec![])
        .await
        .0;
    assert_eq!(
        hash.metadata.unwrap().sha256,
        written.metadata.as_ref().unwrap().sha256
    );
    let mut original = written.clone();
    original.request_id = RequestId::new();
    original.state = "running".into();
    let req = FileSystemRequest {
        execution: ExecutionSelection::User {
            context_ref: ExecutionContextRef::new(),
        },
        request_id: original.request_id,
        path: original.path.clone(),
        operation: write(false, None),
        payload_size: text.len() as u32,
        payload_sha256: Some(filesystem_io::digest(text.as_bytes())),
    };
    svc.store
        .accept_filesystem_with_context(
            actor(),
            &req,
            "reconcile-fixture",
            original.execution_context.as_ref(),
        )
        .await
        .unwrap();
    svc.store.begin_file_publication(&original).await.unwrap();
    let observed = svc
        .for_ui_connection()
        .lookup_filesystem(actor(), original.request_id)
        .await
        .unwrap();
    assert_eq!(observed.state, "completed", "{observed:?}");
    assert_eq!(observed.execution_context, original.execution_context);
    let changed = run(
        &svc,
        user,
        &file,
        write(true, Some("0".repeat(64))),
        b"bad".to_vec(),
        false,
    )
    .await
    .0;
    assert_eq!(changed.error.unwrap().code, "version_conflict");
    assert_eq!(std::fs::read(&file).unwrap(), text.as_bytes());
    let held = svc.upload_locks.try_acquire(&file).await.unwrap().unwrap();
    let busy = run(
        &svc,
        user,
        &file,
        write(true, None),
        b"busy".to_vec(),
        false,
    )
    .await
    .0;
    assert_eq!(busy.error.unwrap().code, "path_busy");
    drop(held);
    let edits = serde_json::to_vec(&vec![TextEdit {
        find: "ORIGINAL".into(),
        replace: "UPDATED".into(),
        expected_matches: 1,
    }])
    .unwrap();
    success(
        &svc,
        user,
        &file,
        FileSystemAction::Patch {
            dry_run: false,
            expected_hash: filesystem_io::digest(text.as_bytes()),
            encoding: None,
        },
        edits,
    )
    .await;
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .starts_with("UPDATED\n")
    );
    let search = success(
        &svc,
        user,
        &root.path,
        FileSystemAction::Search {
            options: Default::default(),
            mode: FileSearchMode::Name,
            query: "data".into(),
            glob: "*".into(),
            case_sensitive: true,
            max_results: 10,
            max_depth: 5,
            max_file_bytes: 131072,
        },
        vec![],
    )
    .await
    .0;
    assert_eq!(search.search.unwrap().matches.len(), 1);
    let copy = root.path.join("copy.txt");
    success(
        &svc,
        user,
        &file,
        FileSystemAction::Copy {
            destination: copy.to_str().unwrap().into(),
            recursive: false,
            overwrite: false,
            limits: Default::default(),
        },
        vec![],
    )
    .await;
    assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&file).unwrap());
    let first = success(
        &svc,
        user,
        &root.path,
        FileSystemAction::ListDirectory {
            after: None,
            limit: 1,
        },
        vec![],
    )
    .await
    .0;
    let page = first.directory.as_ref().unwrap();
    assert_eq!(page.execution_context, first.execution_context);
    assert_eq!(page.entries[0].name, "copy.txt");
    let second = success(
        &svc,
        user,
        &root.path,
        FileSystemAction::ListDirectory {
            after: page.next_after.clone(),
            limit: 1,
        },
        vec![],
    )
    .await
    .0;
    let page = second.directory.unwrap();
    assert_eq!(page.entries[0].name, "中文 data.txt");
    assert!(page.next_after.is_none());
    let moved = root.path.join("moved.txt");
    success(
        &svc,
        user,
        &copy,
        FileSystemAction::Move {
            destination: moved.to_str().unwrap().into(),
            recursive: false,
            overwrite: false,
            limits: Default::default(),
        },
        vec![],
    )
    .await;
    assert!(!copy.exists());
    assert!(moved.exists());
    let archive = root.path.join("archive.zip");
    success(
        &svc,
        user,
        &archive,
        FileSystemAction::ArchiveCreate {
            sources: vec![file.to_str().unwrap().into()],
            overwrite: false,
            limits: Default::default(),
        },
        vec![],
    )
    .await;
    let extracted = root.path.join("extracted");
    success(
        &svc,
        user,
        &archive,
        FileSystemAction::ArchiveExtract {
            destination: extracted.to_str().unwrap().into(),
            overwrite: false,
            max_ratio: 1000,
            limits: Default::default(),
        },
        vec![],
    )
    .await;
    assert_eq!(
        std::fs::read(extracted.join(file.file_name().unwrap())).unwrap(),
        std::fs::read(&file).unwrap()
    );
    let cancelled = run(
        &svc,
        user,
        &moved,
        FileSystemAction::Delete {
            recursive: false,
            limits: Default::default(),
        },
        vec![],
        true,
    )
    .await
    .0;
    // Cancellation can race completion in general; here it is sent before work starts.
    assert_eq!(cancelled.state, "cancelled", "{cancelled:?}");
    assert!(moved.exists());
    success(
        &svc,
        user,
        &moved,
        FileSystemAction::Delete {
            recursive: false,
            limits: Default::default(),
        },
        vec![],
    )
    .await;
    assert!(!moved.exists());
    // A service-owned, private directory must remain inaccessible to this user.
    let private = database.path().join("private");
    std::fs::create_dir(&private).unwrap();
    let hidden = private.join("existing.txt");
    std::fs::write(&hidden, b"service-only").unwrap();
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
    let denied = run(
        &svc,
        user,
        &private.join("must-not-exist.txt"),
        write(false, None),
        b"forbidden".to_vec(),
        false,
    )
    .await
    .0;
    assert_eq!(denied.state, "failed", "{denied:?}");
    let denied_listing = run(
        &svc,
        user,
        &private,
        FileSystemAction::ListDirectory {
            after: None,
            limit: 1,
        },
        vec![],
        false,
    )
    .await
    .0;
    assert_eq!(denied_listing.state, "failed", "{denied_listing:?}");
    assert!(denied_listing.directory.is_none());
    assert!(!private.join("must-not-exist.txt").exists());
    let mut unconfirmed = original.clone();
    unconfirmed.request_id = RequestId::new();
    unconfirmed.path = hidden.to_str().unwrap().into();
    let meta = unconfirmed.metadata.as_mut().unwrap();
    meta.size = 12;
    meta.sha256 = Some(filesystem_io::digest(b"service-only"));
    let req = FileSystemRequest {
        execution: ExecutionSelection::User {
            context_ref: ExecutionContextRef::new(),
        },
        request_id: unconfirmed.request_id,
        path: unconfirmed.path.clone(),
        operation: write(false, None),
        payload_size: 12,
        payload_sha256: meta.sha256.clone(),
    };
    svc.store
        .accept_filesystem_with_context(
            actor(),
            &req,
            "private-reconcile-fixture",
            unconfirmed.execution_context.as_ref(),
        )
        .await
        .unwrap();
    svc.store
        .begin_file_publication(&unconfirmed)
        .await
        .unwrap();
    assert_eq!(
        svc.lookup_filesystem(actor(), unconfirmed.request_id)
            .await
            .unwrap()
            .state,
        "unconfirmed"
    );
    assert!(svc.upload_locks.try_acquire(&file).await.unwrap().is_some());
    assert!(
        !std::fs::read_dir(&root.path)
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with(".pab-"))
    );
}
