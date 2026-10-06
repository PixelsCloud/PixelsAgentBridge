//! Native worker tests; these do not claim public MCP schema/host acceptance.
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
async fn run(
    svc: &TaskService,
    user: u32,
    path: &Path,
    action: FileSystemAction,
    payload: Vec<u8>,
    cancel: bool,
) -> (FileSystemReply, Vec<u8>) {
    let request = FileSystemRequest {
        request_id: RequestId::new(),
        path: path.to_str().unwrap().into(),
        payload_size: payload.len() as u32,
        payload_sha256: action
            .has_payload()
            .then(|| filesystem_io::digest(&payload)),
        operation: action,
    };
    svc.store
        .accept_filesystem(
            actor(),
            &request,
            &filesystem_io::digest(&serde_json::to_vec(&request).unwrap()),
        )
        .await
        .unwrap();
    let (send, receive) = watch::channel(cancel);
    let result = crate::user_worker::filesystem::execute(
        &PathBuf::from(std::env::var_os("PAB_EXECUTION_TEST_WORKER").unwrap()),
        prepared(user),
        request.clone(),
        payload,
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
#[ignore = "requires native user and PAB_EXECUTION_TEST_WORKER; owns only a generated home fixture"]
async fn native_user_filesystem_worker_acceptance() {
    let user: u32 = std::env::var("PAB_EXECUTION_TEST_USER")
        .unwrap()
        .parse()
        .unwrap();
    let identity = prepared(user).identity().clone();
    let database = tempfile::tempdir().unwrap();
    let svc = service(database.path()).await;
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
    assert!(!private.join("must-not-exist.txt").exists());
    assert!(svc.upload_locks.try_acquire(&file).await.unwrap().is_some());
    assert!(
        !std::fs::read_dir(&root.path)
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with(".pab-"))
    );
}
