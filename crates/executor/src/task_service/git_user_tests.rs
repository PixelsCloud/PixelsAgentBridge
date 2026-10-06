use super::transfer_tests::{actor, service};
use super::*;
use pab_protocol::*;
use std::path::{Path, PathBuf};

async fn result(svc: &TaskService, id: RequestId, q: SystemQuery) -> SystemQueryReply {
    let mut r = svc.system_query(actor(), id, q).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(35);
    while r.state == "running" {
        assert!(tokio::time::Instant::now() < deadline, "{r:?}");
        tokio::time::sleep(Duration::from_millis(25)).await;
        r = svc.get_system_query(actor(), id).await.unwrap();
    }
    r
}
async fn command(
    svc: &TaskService,
    selection: ExecutionSelection,
    program: &str,
    args: Vec<String>,
    cwd: Option<&Path>,
) -> String {
    let mut spec = super::execution_tests::spec(svc);
    spec.options.execution = selection;
    spec.program = program.into();
    spec.args = args;
    spec.cwd = cwd.map(|p| p.to_str().unwrap().into());
    let r = svc
        .submit_command(actor(), RequestId::new(), spec)
        .await
        .unwrap();
    let r = super::execution_tests::finished(svc, r.task_ref).await;
    assert_eq!(
        r.state,
        TaskState::Succeeded,
        "fixture command failed: {r:?}"
    );
    let out = svc
        .store
        .read_output(actor(), r.task_ref, OutputStream::Stdout, 0, 8192)
        .await
        .unwrap()
        .0;
    String::from_utf8_lossy(&out.bytes).into()
}
fn query(repo: &Path, selection: ExecutionSelection, action: GitAction) -> SystemQuery {
    SystemQuery::Git {
        query: GitQuery {
            repo: repo.to_str().unwrap().into(),
            execution: selection,
            action,
            timeout_ms: 15000,
        },
    }
}
struct Workspace {
    path: PathBuf,
    home: PathBuf,
}
impl Drop for Workspace {
    fn drop(&mut self) {
        // Delete only this generated fixture directory, after resolving its parent.
        if !self
            .path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("pab-git-acceptance-"))
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

#[tokio::test]
async fn missing_git_user_does_not_accept_service_work() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let q = query(
        dir.path(),
        ExecutionSelection::User {
            context_ref: ExecutionContextRef::new(),
        },
        GitAction::Status { limit: 1 },
    );
    let id = RequestId::new();
    assert!(matches!(
        svc.system_query(actor(), id, q).await,
        Err(TaskServiceError::ExecutionContext(_))
    ));
    assert!(svc.store.get_system_query(actor(), id).await.is_err());
}

#[tokio::test]
#[ignore = "requires PAB_EXECUTION_TEST_WORKER/PAB_EXECUTION_TEST_USER and native Git; owns a generated user-home fixture"]
async fn native_user_git_acceptance() {
    use pab_os_control::execution::PreparedUser;
    let user: u32 = std::env::var("PAB_EXECUTION_TEST_USER")
        .unwrap()
        .parse()
        .unwrap();
    #[cfg(windows)]
    let prepared = PreparedUser::for_session(user).unwrap();
    #[cfg(unix)]
    let prepared = PreparedUser::for_uid(user).unwrap();
    let expected = prepared.identity().clone();
    let dir = tempfile::tempdir().unwrap();
    let base = service(dir.path()).await;
    let mut svc = base.for_ui_connection();
    svc.worker_executable = Some(
        std::env::var_os("PAB_EXECUTION_TEST_WORKER")
            .unwrap()
            .into(),
    );
    let inventory = result(
        &svc,
        RequestId::new(),
        SystemQuery::ExecutionContexts {
            user: Some(expected.account_name.clone()),
            include_system: true,
            limit: 100,
        },
    )
    .await;
    let Some(SystemQueryData::ExecutionContexts { entries, .. }) = inventory.data else {
        panic!("no accounts")
    };
    let selected = entries
        .iter()
        .find(|e| {
            e.mode == ExecutionMode::User
                && e.account_id.as_ref() == Some(&expected.account_id)
                && e.session_id == expected.session_id.map(|n| n.to_string())
        })
        .unwrap()
        .selection
        .unwrap();
    let root = Workspace {
        path: expected
            .home
            .join(format!("pab-git-acceptance-{}", RequestId::new())),
        home: expected.home.clone(),
    };
    let repo = root.path.join("repo");
    let remote = root.path.join("remote.git");
    for args in [
        vec![
            "init".into(),
            "-b".into(),
            "main".into(),
            repo.to_str().unwrap().into(),
        ],
        vec![
            "init".into(),
            "--bare".into(),
            remote.to_str().unwrap().into(),
        ],
    ] {
        command(&svc, selected, "git", args, None).await;
    }
    for (key, value) in [
        ("user.name", "PAB User Fixture"),
        ("user.email", "fixture@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.autocrlf", "false"),
    ] {
        command(
            &svc,
            selected,
            "git",
            vec!["config".into(), key.into(), value.into()],
            Some(&repo),
        )
        .await;
    }
    command(
        &svc,
        selected,
        "git",
        vec![
            "remote".into(),
            "add".into(),
            "origin".into(),
            remote.to_str().unwrap().into(),
        ],
        Some(&repo),
    )
    .await;
    #[cfg(windows)]
    command(
        &svc,
        selected,
        "powershell.exe",
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            "[IO.File]::WriteAllText((Join-Path (Get-Location) 'user.txt'), 'user data')".into(),
        ],
        Some(&repo),
    )
    .await;
    #[cfg(unix)]
    command(
        &svc,
        selected,
        "/bin/sh",
        vec!["-c".into(), "printf 'user data' > user.txt".into()],
        Some(&repo),
    )
    .await;
    let id = RequestId::new();
    let q = query(
        &repo,
        selected,
        GitAction::Commit {
            files: vec!["user.txt".into()],
            message: "user fixture commit".into(),
        },
    );
    let commit = result(&svc, id, q.clone()).await;
    assert_eq!(commit.state, "completed", "{commit:?}");
    assert_eq!(
        commit
            .execution_context
            .as_ref()
            .unwrap()
            .identity
            .as_ref()
            .unwrap()
            .account_id,
        expected.account_id
    );
    assert_eq!(
        svc.system_query(actor(), id, q.clone()).await.unwrap(),
        commit
    );
    assert!(
        svc.system_query(
            actor(),
            id,
            query(
                &repo,
                Default::default(),
                GitAction::Commit {
                    files: vec!["user.txt".into()],
                    message: "user fixture commit".into()
                }
            )
        )
        .await
        .is_err()
    );
    // Holding the service's repository lock must also exclude the user worker.
    let lock = super::git::repo_lock(repo.join(".git").canonicalize().unwrap()).await;
    let held = lock.lock().await;
    let busy = result(
        &svc,
        RequestId::new(),
        query(&repo, selected, GitAction::Status { limit: 10 }),
    )
    .await;
    assert_eq!(busy.state, "failed", "{busy:?}");
    assert!(busy.error.unwrap().contains("git_repository_busy"));
    drop(held);
    let history = result(
        &svc,
        RequestId::new(),
        query(
            &repo,
            selected,
            GitAction::Log {
                start: None,
                skip: 0,
                limit: 10,
            },
        ),
    )
    .await;
    let Some(SystemQueryData::Git { snapshot }) = history.data else {
        panic!("{history:?}")
    };
    assert_eq!(snapshot.commits.len(), 1);
    assert_eq!(snapshot.commits[0].author, "PAB User Fixture");
    let pushed = result(
        &svc,
        RequestId::new(),
        query(
            &repo,
            selected,
            GitAction::Push {
                remote: "origin".into(),
                branch: "main".into(),
                force: false,
            },
        ),
    )
    .await;
    assert_eq!(pushed.state, "completed", "{pushed:?}");
    let oid = command(
        &svc,
        selected,
        "git",
        vec!["rev-parse".into(), "HEAD".into()],
        Some(&repo),
    )
    .await;
    let published = command(
        &svc,
        selected,
        "git",
        vec!["rev-parse".into(), "refs/heads/main".into()],
        Some(&remote),
    )
    .await;
    assert_eq!(oid.trim(), published.trim());
    // Reconciliation must execute as the originally accepted user even when the
    // new network connection no longer holds the old discovery reference.
    let reconcile_id = RequestId::new();
    let push_query = query(
        &repo,
        selected,
        GitAction::Push {
            remote: "origin".into(),
            branch: "main".into(),
            force: false,
        },
    );
    svc.store
        .accept_system_query_with_context(
            actor(),
            reconcile_id,
            &push_query,
            pushed.execution_context.as_ref(),
        )
        .await
        .unwrap();
    let mut lost = pushed.clone();
    lost.request_id = reconcile_id;
    lost.state = "unconfirmed".into();
    svc.store.finish_system_query(&lost).await.unwrap();
    let mut reconnected = base.for_ui_connection();
    reconnected.worker_executable = svc.worker_executable.clone();
    let reconciled = reconnected
        .get_system_query(actor(), reconcile_id)
        .await
        .unwrap();
    assert_eq!(reconciled.state, "completed", "{reconciled:?}");
    assert_eq!(reconciled.execution_context, pushed.execution_context);
    assert_eq!(
        reconnected.system_query(actor(), id, q).await.unwrap(),
        commit
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for path in [
            repo.join("user.txt"),
            repo.join(".git/HEAD"),
            remote.join("refs/heads/main"),
        ] {
            assert_eq!(std::fs::metadata(path).unwrap().uid(), user);
        }
    }
    #[cfg(windows)]
    {
        let owner=command(&svc,selected,"powershell.exe",vec!["-NoProfile".into(),"-NonInteractive".into(),"-Command".into(),"[Security.Principal.WindowsIdentity]::GetCurrent().Owner.Value; $acl=Get-Acl -LiteralPath 'user.txt'; $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value".into()],Some(&repo)).await;
        let owners = owner.lines().map(str::trim).collect::<Vec<_>>();
        assert_eq!(owners.len(), 2);
        assert_eq!(
            owners[0], owners[1],
            "file owner must match target token default owner"
        );
        assert_ne!(owners[1], "S-1-5-18");
    }
}
