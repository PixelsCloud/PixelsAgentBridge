use super::*;

#[test]
fn output_budget_preserves_absolute_offsets_and_cwd_checks_mounts() {
    let mut output = Output::default();
    output.append(&vec![42; OUTPUT_LIMIT + 123]);
    assert_eq!(
        output.range(false),
        OutputRange {
            retained_from: 123,
            available_to: (OUTPUT_LIMIT + 123) as u64,
            complete: false
        }
    );
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("child")).unwrap();
    assert_eq!(
        resolve_cwd(temp.path(), "/workspace/child").unwrap(),
        temp.path().join("child")
    );
    assert!(resolve_cwd(temp.path(), "/workspace-other").is_err());
    assert!(resolve_cwd(temp.path(), "/workspace/../private").is_err());
    assert!(resolve_cwd(temp.path(), "/shared-abc").is_err());
}

fn device() -> DeviceRef {
    DeviceRef {
        tenant_id: TenantId::new(),
        device_id: DeviceId::new(),
    }
}
fn peer(device: DeviceRef) -> AuthorizedDevicePeer {
    let key = EndpointKey::new([8; 32]);
    AuthorizedDevicePeer {
        device_ref: device,
        operator: OperatorRef::guest(key),
        peer_endpoint_key: key,
        user_context: EndpointUserContext {
            revision: 1,
            user: None,
            policy_version: 1,
        },
        authorized_at_unix_ms: now_ms(),
    }
}
fn context() -> ExecutionContext {
    crate::peer::context(&crate::peer::RuntimeOptions {
        database_file: "/tmp/mobile.sqlite".into(),
        password_hash: String::new(),
        password_version: 1,
        android_release: "9".into(),
        android_sdk: 28,
        storage_staging_directory: Some("/tmp/staging".into()),
    })
    .unwrap()
}
fn spec(context: &ExecutionContext) -> CommandTaskSpec {
    #[cfg(windows)]
    let (program, args) = (
        "C:\\Windows\\System32\\cmd.exe",
        vec!["/D", "/C", "echo", "SHARED_COMMAND_42"],
    );
    #[cfg(not(windows))]
    let (program, args) = ("/bin/sh", vec!["-c", "printf SHARED_COMMAND_42"]);
    CommandTaskSpec {
        options: CommandOptions::default(),
        program: program.into(),
        args: args.into_iter().map(String::from).collect(),
        cwd: Some("/workspace".into()),
        expected_environment: ExpectedEnvironment {
            os_family: context.os_family,
            environment_revision: context.environment_revision.clone(),
        },
        display_summary: "owned command test".into(),
    }
}
async fn gate(service: Arc<ExecutionService>, root: PathBuf) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let job = service.sdk.poll().await;
            if job.is_null() {
                tokio::time::sleep(Duration::from_millis(5)).await;
                continue;
            }
            assert_eq!(job["command"]["action"], "authorize");
            service
                .sdk
                .complete(
                    serde_json::from_value(job["ticket"].clone()).unwrap(),
                    json!({"workspace":root,"uid":12345}),
                )
                .await;
        }
    })
}

#[tokio::test]
async fn command_output_owner_dedup_conflict_restart_never_reexecutes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    let path = temp.path().join("execution.sqlite");
    let device = device();
    let peer = peer(device);
    let context = context();
    let command = spec(&context);
    let id = RequestId::new();
    let service = ExecutionService::open(&path, [3; 32], device)
        .await
        .unwrap();
    let gate = gate(service.clone(), root).await;
    let accepted = service
        .submit("first", &peer, &context, id, command.clone())
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    let finished = loop {
        let current = service.snapshot("first", accepted.task_ref).await.unwrap();
        if current.state.is_terminal() {
            break current;
        }
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(finished.state, TaskState::Succeeded, "{finished:?}");
    let output = service
        .output(
            "first",
            accepted.task_ref,
            OutputStream::Stdout,
            0,
            MAX_OUTPUT_READ_BYTES,
        )
        .await
        .unwrap();
    let DeviceTaskResponse::Output { chunk, range } = output else {
        panic!("output")
    };
    assert!(String::from_utf8_lossy(&chunk.bytes).contains("SHARED_COMMAND_42"));
    assert!(range.complete);
    assert_eq!(
        service
            .submit("first", &peer, &context, id, command.clone())
            .await
            .unwrap()
            .task_ref,
        accepted.task_ref
    );
    assert!(matches!(
        service.snapshot("other", accepted.task_ref).await,
        Err(ExecError(DeviceTaskErrorCode::NotFound, _))
    ));
    assert!(matches!(
        service.cancel("other", accepted.task_ref, "other").await,
        Err(ExecError(DeviceTaskErrorCode::NotFound, _))
    ));
    let mut conflict = command.clone();
    conflict
        .options
        .env
        .insert("CHANGED".into(), "value".into());
    assert!(matches!(
        service.submit("first", &peer, &context, id, conflict).await,
        Err(ExecError(DeviceTaskErrorCode::RequestConflict, _))
    ));
    gate.abort();
    let _ = gate.await;
    service.shutdown().await;
    drop(service);
    let restored = ExecutionService::open(&path, [3; 32], device)
        .await
        .unwrap();
    // No SDK responder exists after restart: a duplicate must return the fact.
    let original = restored
        .submit("first", &peer, &context, id, command)
        .await
        .unwrap();
    assert_eq!(original.state, TaskState::Succeeded);
    assert_eq!(
        original.output.stdout.retained_from,
        original.output.stdout.available_to
    );
    assert!(restored.sdk.pending_is_empty().await);
    restored.shutdown().await;
}

#[tokio::test]
async fn restart_interrupts_inflight_fact_and_keeps_failed_terminal_tombstone() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("execution.sqlite");
    let device = device();
    let peer = peer(device);
    let context = context();
    let command = spec(&context);
    let id = RequestId::new();
    let service = ExecutionService::open(&path, [3; 32], device)
        .await
        .unwrap();
    let (aggregate, _) = TaskAggregate::accept(AcceptedTask {
        task_ref: TaskRef {
            device_ref: device,
            task_id: TaskId::new(),
        },
        request_id: id,
        initiated_by: peer.operator,
        capability: CapabilityRef {
            name: "command".into(),
            version: 2,
        },
        display_summary: "accepted before crash".into(),
        execution_context: context.clone(),
        accepted_at_unix_ms: now_ms(),
    })
    .unwrap();
    let snapshot = aggregate.snapshot().clone();
    sqlx::query("INSERT INTO command_tasks(owner,id,task,digest,snapshot) VALUES('owner',?,?,?,?)")
        .bind(id.to_string())
        .bind(snapshot.task_ref.task_id.to_string())
        .bind(service.digest(&serde_json::to_vec(&command).unwrap()))
        .bind(json!(snapshot).to_string())
        .execute(&service.pool)
        .await
        .unwrap();
    let term = RequestId::new();
    sqlx::query(
        "INSERT INTO terminals(owner,id,digest,state) VALUES('owner',?,'original','running')",
    )
    .bind(term.to_string())
    .execute(&service.pool)
    .await
    .unwrap();
    drop(service);
    let service = ExecutionService::open(&path, [3; 32], device)
        .await
        .unwrap();
    let original = service
        .submit("owner", &peer, &context, id, command)
        .await
        .unwrap();
    assert_eq!(original.state, TaskState::Interrupted);
    assert!(original.output.stdout.complete);
    let state: String = sqlx::query_scalar("SELECT state FROM terminals WHERE id=?")
        .bind(term.to_string())
        .fetch_one(&service.pool)
        .await
        .unwrap();
    assert_eq!(state, "interrupted");
    assert!(service.sdk.pending_is_empty().await);
    service.shutdown().await;
}

#[tokio::test]
async fn dropping_transport_waiter_cannot_abandon_accepted_dispatch() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    let service = ExecutionService::open(&temp.path().join("execution.sqlite"), [3; 32], device())
        .await
        .unwrap();
    let context = context();
    let peer = peer(service.device);
    let command = spec(&context);
    let id = RequestId::new();
    let outer = service.clone();
    let worker = service.clone();
    let original = command.clone();
    let actor = peer.clone();
    let expected = context.clone();
    let waiter = tokio::spawn(async move {
        outer
            .detached(async move {
                worker
                    .submit("owner", &actor, &expected, id, original)
                    .await
            })
            .await
    });
    let job = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let job = service.sdk.poll().await;
            if !job.is_null() {
                break job;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    waiter.abort();
    let _ = waiter.await;
    service
        .sdk
        .complete(
            serde_json::from_value(job["ticket"].clone()).unwrap(),
            json!({"workspace":root,"uid":12345}),
        )
        .await;
    let gate = gate(service.clone(), root).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM command_tasks")
            .fetch_one(&service.pool)
            .await
            .unwrap();
        if rows == 1 {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let accepted = service
        .submit("owner", &peer, &context, id, command)
        .await
        .unwrap();
    loop {
        let snapshot = service.snapshot("owner", accepted.task_ref).await.unwrap();
        if snapshot.state.is_terminal() {
            assert_eq!(snapshot.state, TaskState::Succeeded);
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM command_tasks")
        .fetch_one(&service.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    gate.abort();
    let _ = gate.await;
    service.shutdown().await;
}

#[tokio::test]
async fn live_output_completion_is_not_regressed_by_an_older_journal_read() {
    let temp = tempfile::tempdir().unwrap();
    let service = ExecutionService::open(&temp.path().join("execution.sqlite"), [7; 32], device())
        .await
        .unwrap();
    let actor = peer(service.device);
    let id = RequestId::new();
    let task_ref = TaskRef {
        device_ref: service.device,
        task_id: TaskId::new(),
    };
    let (mut aggregate, _) = TaskAggregate::accept(AcceptedTask {
        task_ref,
        request_id: id,
        initiated_by: actor.operator,
        capability: CapabilityRef {
            name: "process.exec".into(),
            version: 1,
        },
        display_summary: "output completion race".into(),
        execution_context: context(),
        accepted_at_unix_ms: now_ms(),
    })
    .unwrap();
    aggregate.record(TaskEventKind::Running, now_ms()).unwrap();
    let old = aggregate.snapshot().clone();
    sqlx::query("INSERT INTO command_tasks(owner,id,task,digest,snapshot) VALUES('owner',?,?,?,?)")
        .bind(id.to_string())
        .bind(task_ref.task_id.to_string())
        .bind("owned-test")
        .bind(json!(old).to_string())
        .execute(&service.pool)
        .await
        .unwrap();
    let mut stdout = Output::default();
    stdout.append("中文".as_bytes());
    aggregate
        .observe_output(OutputStream::Stdout, 0, 6, true)
        .unwrap();
    let (cancel, _receiver) = watch::channel(None);
    service.live.lock().await.insert(
        task_ref.task_id,
        Arc::new(Mutex::new(LiveTask {
            aggregate,
            stdout,
            stderr: Output::default(),
            cancel,
        })),
    );
    let live = service
        .live
        .lock()
        .await
        .get(&task_ref.task_id)
        .unwrap()
        .clone();
    let sink = TaskSink {
        service: service.clone(),
        task: live,
    };
    assert!(
        pab_command_runtime::CommandSink::output(&sink, OutputStream::Stdout, b"late")
            .await
            .is_err()
    );
    let reply = service
        .output("owner", task_ref, OutputStream::Stdout, 0, 8192)
        .await
        .unwrap();
    let DeviceTaskResponse::Output { chunk, range } = reply else {
        panic!("output")
    };
    assert!(range.complete);
    assert_eq!(chunk.bytes, "中文".as_bytes());
    service.live.lock().await.clear();
    service.shutdown().await;
}
