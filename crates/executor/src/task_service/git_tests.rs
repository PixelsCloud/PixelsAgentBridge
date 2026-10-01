use super::super::transfer_tests::{actor, pair, service};
use super::*;
use std::process::Command as StdCommand;

struct Fixture {
    _dir: tempfile::TempDir,
    repo: PathBuf,
}
fn git(repo: &Path, args: &[&str]) -> String {
    let mut cmd = StdCommand::new("git");
    cmd.current_dir(repo)
        .args(["-c", "core.quotepath=false"])
        .args(args)
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "{:?}: {}",
        args,
        text(&output.stderr)
    );
    text(&output.stdout)
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("仓库 with space");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.name", "测试用户"]);
        git(&repo, &["config", "user.email", "fixture@example.invalid"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        git(&repo, &["config", "core.autocrlf", "false"]);
        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        std::fs::write(repo.join("other.txt"), "other\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "初始提交"]);
        Self { _dir: dir, repo }
    }
    fn spec(&self, action: GitAction) -> GitQuery {
        GitQuery {
            repo: self.repo.to_string_lossy().into(),
            action,
            timeout_ms: 10_000,
        }
    }
    async fn invoke(&self, action: GitAction) -> SystemQueryReply {
        let (_send, receive) = watch::channel(false);
        query(RequestId::new(), &self.spec(action), receive, None).await
    }
}
fn snapshot(r: &SystemQueryReply) -> &GitSnapshot {
    let Some(SystemQueryData::Git { snapshot }) = &r.data else {
        panic!("missing Git data: {r:?}")
    };
    snapshot
}
fn diff() -> GitAction {
    GitAction::Diff {
        staged: false,
        base: None,
        head: None,
        paths: vec![],
        context_lines: 3,
        max_bytes: 16384,
    }
}

#[tokio::test]
async fn status_diff_and_stable_history_cover_unicode_rename_binary_and_detached_head() {
    let f = Fixture::new();
    std::fs::write(f.repo.join("base.txt"), "变化中文\n").unwrap();
    std::fs::write(f.repo.join("新 文件.txt"), "new").unwrap();
    git(&f.repo, &["mv", "other.txt", "改名 文件.txt"]);
    let r = f.invoke(GitAction::Status { limit: 100 }).await;
    assert_eq!(r.state, "completed", "{r:?}");
    let s = snapshot(&r);
    assert_eq!(s.branch.as_deref(), Some("main"));
    assert!(
        s.entries
            .iter()
            .any(|e| e.path == "新 文件.txt" && e.untracked)
    );
    assert!(
        s.entries
            .iter()
            .any(|e| e.path == "改名 文件.txt" && e.previous_path.as_deref() == Some("other.txt"))
    );
    let r = f.invoke(diff()).await;
    assert_eq!(r.state, "completed");
    assert!(snapshot(&r).diff.as_ref().unwrap().contains("变化中文"));
    let r = f
        .invoke(GitAction::Log {
            start: None,
            skip: 0,
            limit: 1,
        })
        .await;
    let anchor = snapshot(&r).start_commit.clone().unwrap();
    assert_eq!(snapshot(&r).commits[0].subject, "初始提交");
    git(&f.repo, &["add", "."]);
    git(&f.repo, &["commit", "-m", "second"]);
    let old = f
        .invoke(GitAction::Log {
            start: Some(anchor.clone()),
            skip: 0,
            limit: 20,
        })
        .await;
    assert_eq!(snapshot(&old).commits.len(), 1);
    assert_eq!(snapshot(&old).commits[0].id, anchor);
    let r = f
        .invoke(GitAction::Checkout {
            reference: anchor,
            detach: true,
        })
        .await;
    assert_eq!(r.state, "completed", "{r:?}");
    let r = f.invoke(GitAction::Status { limit: 100 }).await;
    assert!(snapshot(&r).detached);
    assert!(snapshot(&r).branch.is_none());
    std::fs::write(f.repo.join("base.txt"), [0, 1, 2, 3]).unwrap();
    let r = f.invoke(diff()).await;
    assert!(snapshot(&r).diff.as_ref().unwrap().contains("Binary files"));
}
#[tokio::test]
async fn selected_commit_preserves_unrelated_staging_and_handles_index_lock() {
    let f = Fixture::new();
    std::fs::create_dir(f.repo.join("folder")).unwrap();
    std::fs::write(
        f.repo.join("folder/unselected.txt"),
        "never stage directory implicitly",
    )
    .unwrap();
    let directory = f
        .invoke(GitAction::Commit {
            files: vec!["folder".into()],
            message: "directory selection".into(),
        })
        .await;
    assert_eq!(directory.state, "failed");
    assert!(git(&f.repo, &["diff", "--cached", "--name-only"]).is_empty());
    std::fs::write(f.repo.join("other.txt"), "staged unrelated\n").unwrap();
    git(&f.repo, &["add", "other.txt"]);
    std::fs::write(f.repo.join("新 文件.txt"), "selected\n").unwrap();
    let r = f
        .invoke(GitAction::Commit {
            files: vec!["新 文件.txt".into()],
            message: "仅选中文文件".into(),
        })
        .await;
    assert_eq!(r.state, "completed", "{r:?}");
    let changed = git(
        &f.repo,
        &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
    );
    assert_eq!(changed, "新 文件.txt");
    assert_eq!(
        git(&f.repo, &["diff", "--cached", "--name-only"]),
        "other.txt"
    );
    std::fs::remove_file(f.repo.join("base.txt")).unwrap();
    let removed = f
        .invoke(GitAction::Commit {
            files: vec!["base.txt".into()],
            message: "explicit tracked deletion".into(),
        })
        .await;
    assert_eq!(removed.state, "completed", "{removed:?}");
    assert_eq!(
        git(
            &f.repo,
            &["diff-tree", "--no-commit-id", "--name-status", "-r", "HEAD"]
        ),
        "D\tbase.txt"
    );
    assert_eq!(
        git(&f.repo, &["diff", "--cached", "--name-only"]),
        "other.txt"
    );
    let before = git(&f.repo, &["rev-parse", "HEAD"]);
    std::fs::write(f.repo.join(".git/index.lock"), "fixture lock").unwrap();
    std::fs::write(f.repo.join("base.txt"), "blocked").unwrap();
    let r = f
        .invoke(GitAction::Commit {
            files: vec!["base.txt".into()],
            message: "locked".into(),
        })
        .await;
    assert_eq!(r.state, "failed");
    assert_eq!(git(&f.repo, &["rev-parse", "HEAD"]), before);
    assert!(f.repo.join(".git/index.lock").exists());
}
#[tokio::test]
async fn empty_repository_bad_references_truncation_and_checkout_preserve_changes() {
    let f = Fixture::new();
    git(&f.repo, &["branch", "other"]);
    std::fs::write(f.repo.join("base.txt"), "committed other\n").unwrap();
    git(&f.repo, &["add", "base.txt"]);
    git(&f.repo, &["commit", "-m", "main diverged"]);
    std::fs::write(f.repo.join("base.txt"), "keep my edits\n").unwrap();
    let r = f
        .invoke(GitAction::Checkout {
            reference: "other".into(),
            detach: false,
        })
        .await;
    assert_eq!(r.state, "failed");
    assert_eq!(
        std::fs::read_to_string(f.repo.join("base.txt")).unwrap(),
        "keep my edits\n"
    );
    let r = f
        .invoke(GitAction::Log {
            start: Some("missing-ref".into()),
            skip: 0,
            limit: 10,
        })
        .await;
    assert_eq!(r.state, "failed");
    std::fs::write(f.repo.join("base.txt"), "中文🙂\n".repeat(4000)).unwrap();
    let mut action = diff();
    if let GitAction::Diff { max_bytes, .. } = &mut action {
        *max_bytes = 1024;
    }
    let r = f.invoke(action).await;
    assert_eq!(r.state, "completed");
    assert!(r.truncated);
    assert!(snapshot(&r).diff.as_ref().unwrap().len() <= 1024);
    assert!(serde_json::to_vec(&r).unwrap().len() <= MAX_SYSTEM_REPLY_BYTES);
    for n in 0..4 {
        std::fs::write(f.repo.join(format!("untracked{n}")), "x").unwrap();
    }
    let r = f.invoke(GitAction::Status { limit: 1 }).await;
    assert!(r.truncated);
    assert_eq!(r.returned_count, 1);
    let empty = f._dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    git(&empty, &["init", "-b", "main"]);
    let (_send, receive) = watch::channel(false);
    let r = query(
        RequestId::new(),
        &GitQuery {
            repo: empty.to_string_lossy().into(),
            action: GitAction::Log {
                start: None,
                skip: 0,
                limit: 20,
            },
            timeout_ms: 10000,
        },
        receive,
        None,
    )
    .await;
    assert_eq!(r.state, "completed", "{r:?}");
    assert!(snapshot(&r).unborn);
    assert!(snapshot(&r).commits.is_empty());
}
#[tokio::test]
async fn local_bare_push_fetch_pull_rejection_and_force_lease_are_observed() {
    let f = Fixture::new();
    let bare = f._dir.path().join("remote.git");
    git(f._dir.path(), &["init", "--bare", bare.to_str().unwrap()]);
    git(
        &f.repo,
        &["remote", "add", "origin", bare.to_str().unwrap()],
    );
    let push = || GitAction::Push {
        remote: "origin".into(),
        branch: "main".into(),
        force: false,
    };
    let r = f.invoke(push()).await;
    assert_eq!(r.state, "completed", "{r:?}");
    let first = git(&bare, &["rev-parse", "refs/heads/main"]);
    assert_eq!(Some(first.clone()), snapshot(&r).start_commit);
    let second = f._dir.path().join("second");
    git(
        f._dir.path(),
        &[
            "clone",
            "-b",
            "main",
            bare.to_str().unwrap(),
            second.to_str().unwrap(),
        ],
    );
    git(&second, &["config", "user.name", "fixture"]);
    git(
        &second,
        &["config", "user.email", "fixture@example.invalid"],
    );
    git(&second, &["config", "commit.gpgsign", "false"]);
    std::fs::write(second.join("base.txt"), "remote change\n").unwrap();
    git(&second, &["add", "base.txt"]);
    git(&second, &["commit", "-m", "remote"]);
    git(&second, &["push", "origin", "main"]);
    let r = f.invoke(push()).await;
    assert_eq!(r.state, "failed");
    assert_ne!(git(&bare, &["rev-parse", "main"]), first);
    let r = f
        .invoke(GitAction::Fetch {
            remote: "origin".into(),
            branch: Some("main".into()),
        })
        .await;
    assert_eq!(r.state, "completed", "{r:?}");
    let r = f
        .invoke(GitAction::Pull {
            remote: "origin".into(),
            branch: "main".into(),
            strategy: GitPullStrategy::FfOnly,
        })
        .await;
    assert_eq!(r.state, "completed", "{r:?}");
    assert_eq!(
        std::fs::read_to_string(f.repo.join("base.txt")).unwrap(),
        "remote change\n"
    );
    git(&f.repo, &["reset", "--hard", &first]);
    let r = f
        .invoke(GitAction::Push {
            remote: "origin".into(),
            branch: "main".into(),
            force: true,
        })
        .await;
    assert_eq!(r.state, "completed", "{r:?}");
    assert_eq!(git(&bare, &["rev-parse", "main"]), first);
}
#[tokio::test]
async fn git_requests_are_owned_deduplicated_and_never_replayed_after_restart() {
    let f = Fixture::new();
    let svc = service(f._dir.path()).await;
    let id = RequestId::new();
    let spec = SystemQuery::Git {
        query: f.spec(GitAction::Status { limit: 100 }),
    };
    let r = svc.system_query(actor(), id, spec.clone()).await.unwrap();
    assert_eq!(r.state, "completed");
    std::fs::write(f.repo.join("base.txt"), "after sample").unwrap();
    assert_eq!(
        svc.system_query(actor(), id, spec.clone()).await.unwrap(),
        r
    );
    let foreign = OperatorRef::guest(EndpointKey::new([99; 32]));
    assert!(svc.get_system_query(foreign, id).await.is_err());
    assert!(svc.cancel_system_query(foreign, id).await.is_err());
    let new = SystemQuery::Git {
        query: f.spec(diff()),
    };
    assert!(svc.system_query(actor(), id, new).await.is_err());
    let restart_id = RequestId::new();
    let pending = SystemQuery::Git {
        query: f.spec(GitAction::Commit {
            files: vec!["base.txt".into()],
            message: "never replay".into(),
        }),
    };
    svc.store
        .accept_system_query(actor(), restart_id, &pending)
        .await
        .unwrap();
    let reopened = service(f._dir.path()).await;
    let r = reopened
        .system_query(actor(), restart_id, pending)
        .await
        .unwrap();
    assert_eq!(r.state, "unconfirmed");
    assert!(!git(&f.repo, &["log", "-1", "--format=%s"]).contains("never replay"));
}
#[tokio::test]
async fn async_fetch_cancellation_is_owned_and_keeps_unconfirmed_effects() {
    let f = Fixture::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    git(
        &f.repo,
        &[
            "remote",
            "add",
            "slow",
            &format!("git://127.0.0.1:{port}/fixture"),
        ],
    );
    let svc = service(f._dir.path()).await;
    let id = RequestId::new();
    let spec = SystemQuery::Git {
        query: f.spec(GitAction::Fetch {
            remote: "slow".into(),
            branch: None,
        }),
    };
    let r = svc.system_query(actor(), id, spec.clone()).await.unwrap();
    assert_eq!(r.state, "running");
    let (socket, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let duplicate = svc.system_query(actor(), id, spec).await.unwrap();
    assert_eq!(duplicate.state, "running");
    let foreign = OperatorRef::guest(EndpointKey::new([99; 32]));
    assert!(svc.cancel_system_query(foreign, id).await.is_err());
    assert_eq!(
        svc.cancel_system_query(actor(), id).await.unwrap().state,
        "cancel_requested"
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let r = svc.get_system_query(actor(), id).await.unwrap();
        if r.state != "running" {
            assert_eq!(r.state, "unconfirmed", "{r:?}");
            assert!(!snapshot(&r).command_completed);
            break;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    drop(socket);
}
#[test]
fn machine_output_parser_preserves_raw_paths_and_rejects_incomplete_records() {
    let records = b"R  new name\0old name\0?? \xff.bin\0UU conflict.txt\0";
    let s = parse_status(records).unwrap();
    assert_eq!(s[0].previous_path.as_deref(), Some("old name"));
    assert_eq!(s[1].path_hex.as_deref(), Some("ff2e62696e"));
    assert!(s[2].conflict);
    assert!(parse_status(b"R  missing\0").is_err());
    assert!(parse_status(b"?? truncated").is_err());
    assert!(parse_log(b"not-a-commit\0").is_err());
}
#[tokio::test]
async fn literal_file_selection_repo_busy_missing_git_and_merge_conflicts_are_explicit() {
    let f = Fixture::new();
    std::fs::write(f.repo.join("[abc].txt"), "literal").unwrap();
    std::fs::write(f.repo.join("a.txt"), "unrelated").unwrap();
    let r = f
        .invoke(GitAction::Commit {
            files: vec!["[abc].txt".into()],
            message: "literal paths".into(),
        })
        .await;
    assert_eq!(r.state, "completed", "{r:?}");
    assert!(
        !git(&f.repo, &["ls-tree", "--name-only", "HEAD"])
            .lines()
            .any(|p| p == "a.txt")
    );
    let path = tokio::fs::canonicalize(f.repo.join(".git")).await.unwrap();
    let lock = repo_lock(path).await;
    let held = lock.lock().await;
    let r = f
        .invoke(GitAction::Checkout {
            reference: "main".into(),
            detach: false,
        })
        .await;
    assert_eq!(r.state, "failed");
    assert!(r.error.unwrap().contains("git_repository_busy"));
    drop(held);
    let (_send, cancelled) = watch::channel(false);
    let spec = f.spec(GitAction::Status { limit: 1 });
    let system = SystemQuery::Git { query: spec };
    let mut runner = Runner {
        program: f.repo.join("missing-git-program.exe"),
        cwd: f.repo.clone(),
        deadline: Instant::now() + Duration::from_secs(1),
        cancelled,
        r: SystemQueryReply::pending(RequestId::new(), &system),
        snapshot: GitSnapshot::default(),
        store: None,
        effects_started: false,
    };
    let error = match runner.run(&["status".into()], false).await {
        Err(e) => e,
        Ok(_) => panic!("missing executable started"),
    };
    assert!(error.message.contains("git_not_installed"));
    git(&f.repo, &["branch", "side"]);
    std::fs::write(f.repo.join("base.txt"), "main\n").unwrap();
    git(&f.repo, &["add", "base.txt"]);
    git(&f.repo, &["commit", "-m", "main changes"]);
    git(&f.repo, &["switch", "side"]);
    std::fs::write(f.repo.join("base.txt"), "side\n").unwrap();
    git(&f.repo, &["add", "base.txt"]);
    git(&f.repo, &["commit", "-m", "side changes"]);
    git(&f.repo, &["switch", "main"]);
    let local_remote = f._dir.path().join("local.git");
    git(
        f._dir.path(),
        &["init", "--bare", local_remote.to_str().unwrap()],
    );
    git(
        &f.repo,
        &["remote", "add", "origin", local_remote.to_str().unwrap()],
    );
    git(&f.repo, &["push", "origin", "side"]);
    // Remove the intentionally unrelated untracked fixture before clean-tree pull.
    std::fs::remove_file(f.repo.join("a.txt")).unwrap();
    let r = f
        .invoke(GitAction::Pull {
            remote: "origin".into(),
            branch: "side".into(),
            strategy: GitPullStrategy::Merge,
        })
        .await;
    assert_eq!(r.state, "failed", "{r:?}");
    assert!(f.repo.join(".git/MERGE_HEAD").exists());
    assert!(snapshot(&r).index_may_have_changed);
    let r = f.invoke(GitAction::Status { limit: 100 }).await;
    assert!(snapshot(&r).entries.iter().any(|e| e.conflict));
}
#[tokio::test]
async fn lost_push_receipt_reconciles_exact_remote_state_without_replaying_push() {
    let f = Fixture::new();
    let remote = f._dir.path().join("remote.git");
    git(f._dir.path(), &["init", "--bare", remote.to_str().unwrap()]);
    git(
        &f.repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let action = GitAction::Push {
        remote: "origin".into(),
        branch: "main".into(),
        force: false,
    };
    let actual = f.invoke(action.clone()).await;
    assert_eq!(actual.state, "completed");
    let svc = service(f._dir.path()).await;
    let id = RequestId::new();
    let spec = SystemQuery::Git {
        query: f.spec(action.clone()),
    };
    svc.store
        .accept_system_query(actor(), id, &spec)
        .await
        .unwrap();
    let mut lost = actual.clone();
    lost.request_id = id;
    lost.state = "unconfirmed".into();
    if let Some(SystemQueryData::Git { snapshot }) = &mut lost.data {
        snapshot.command_completed = false;
        snapshot.phase = "pushing".into();
    }
    svc.store.finish_system_query(&lost).await.unwrap();
    let reconciled = svc.get_system_query(actor(), id).await.unwrap();
    assert_eq!(reconciled.state, "completed", "{reconciled:?}");
    assert_eq!(
        snapshot(&reconciled).remote_oid_after,
        snapshot(&actual).start_commit
    );
    assert!(!snapshot(&reconciled).command_completed);
    let other = RequestId::new();
    svc.store
        .accept_system_query(actor(), other, &spec)
        .await
        .unwrap();
    lost.request_id = other;
    svc.store.finish_system_query(&lost).await.unwrap();
    git(&remote, &["update-ref", "-d", "refs/heads/main"]);
    let unresolved = svc.get_system_query(actor(), other).await.unwrap();
    assert_eq!(unresolved.state, "unconfirmed");
    assert!(snapshot(&unresolved).remote_observed_at_unix_ms.is_some());
    assert!(snapshot(&unresolved).remote_oid_after.is_none());
    assert!(
        git(
            &remote,
            &["for-each-ref", "--format=%(objectname)", "refs/heads/main"]
        )
        .is_empty()
    );
}
#[tokio::test]
async fn original_git_sample_crosses_real_quic_and_remains_stable_after_file_changes() {
    let f = Fixture::new();
    let svc = service(f._dir.path()).await;
    let (a, b, client, server) = pair().await;
    let id = RequestId::new();
    let mut original = None;
    for req in [
        DeviceTaskRequest::GetEnvironment {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
        },
        DeviceTaskRequest::SystemQuery {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
            request_id: id,
            query: SystemQuery::Git {
                query: f.spec(GitAction::Status { limit: 100 }),
            },
        },
        DeviceTaskRequest::GetSystemQuery {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
            request_id: id,
        },
    ] {
        let svc = svc.clone();
        let server = server.clone();
        let worker = tokio::spawn(async move {
            svc.handle_stream(
                actor(),
                server.accept_bi(Duration::from_secs(5)).await.unwrap(),
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        });
        let mut stream = client.open_bi(Duration::from_secs(5)).await.unwrap();
        stream
            .send_json(&req, Duration::from_secs(5))
            .await
            .unwrap();
        let response: DeviceTaskResponse =
            stream.receive_json(Duration::from_secs(5)).await.unwrap();
        match response {
            DeviceTaskResponse::Environment {
                system_query_schema_version,
                ..
            } => assert_eq!(
                system_query_schema_version,
                Some(SYSTEM_QUERY_SCHEMA_VERSION)
            ),
            DeviceTaskResponse::SystemQuery { reply } => {
                assert_eq!(reply.state, "completed");
                assert_eq!(reply.kind, "git_status");
                if let Some(prior) = &original {
                    assert_eq!(&reply, prior);
                } else {
                    original = Some(reply);
                    std::fs::write(f.repo.join("base.txt"), "after sample").unwrap();
                }
            }
            other => panic!("unexpected response {other:?}"),
        }
        stream
            .expect_receive_end(Duration::from_secs(5))
            .await
            .unwrap();
        worker.await.unwrap();
    }
    a.close().await;
    b.close().await;
}
#[tokio::test]
async fn byte_truncated_history_pagination_never_skips_unreturned_commits() {
    let f = Fixture::new();
    for i in 0..4 {
        git(
            &f.repo,
            &[
                "commit",
                "--allow-empty",
                "-m",
                &format!("{i} {}", "中文🙂".repeat(600)),
            ],
        );
    }
    let mut skip = 0;
    let mut anchor = None;
    let mut seen = vec![];
    loop {
        let r = f
            .invoke(GitAction::Log {
                start: anchor.clone(),
                skip,
                limit: 5,
            })
            .await;
        assert_eq!(r.state, "completed", "{r:?}");
        let s = snapshot(&r);
        anchor = s.start_commit.clone();
        seen.extend(s.commits.iter().map(|c| c.id.clone()));
        if let Some(next) = s.next_skip {
            assert_eq!(next, skip + r.returned_count);
            skip = next;
        } else {
            break;
        }
        assert!(seen.len() <= 5);
    }
    assert_eq!(
        seen,
        git(&f.repo, &["rev-list", anchor.as_deref().unwrap()])
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    );
}
#[tokio::test]
async fn a_started_fetch_deadline_does_not_claim_rollback_or_reexecute() {
    let f = Fixture::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    git(
        &f.repo,
        &[
            "remote",
            "add",
            "slow",
            &format!("git://127.0.0.1:{port}/fixture"),
        ],
    );
    let (_send, cancelled) = watch::channel(false);
    let mut q = f.spec(GitAction::Fetch {
        remote: "slow".into(),
        branch: None,
    });
    q.timeout_ms = 2000;
    let worker = tokio::spawn(async move { query(RequestId::new(), &q, cancelled, None).await });
    let (socket, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let r = tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.state, "unconfirmed", "{r:?}");
    assert!(r.error.as_ref().unwrap().contains("deadline"));
    assert!(!snapshot(&r).command_completed);
    drop(socket);
}
