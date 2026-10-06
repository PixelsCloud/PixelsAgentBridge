use super::transfer_tests::{actor, service};
use super::*;
use pab_protocol::{
    ExecutionContextRef, ExecutionSelection, ExpectedEnvironment, RequestId, TaskState,
};

pub(super) fn spec(svc: &TaskService) -> CommandTaskSpec {
    CommandTaskSpec {
        program: if cfg!(windows) {
            "C:\\Windows\\System32\\whoami.exe"
        } else {
            "/usr/bin/id"
        }
        .into(),
        args: vec![],
        cwd: None,
        options: Default::default(),
        display_summary: "user fixture".into(),
        expected_environment: ExpectedEnvironment {
            os_family: svc.execution_context.os_family,
            environment_revision: svc.execution_context.environment_revision.clone(),
        },
    }
}
pub(super) async fn finished(svc: &TaskService, task: TaskRef) -> TaskSnapshot {
    tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            let value = svc.store.get_task(actor(), task).await.unwrap();
            if value.state.is_terminal() {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn user_command_missing_context_never_falls_back_and_replay_keeps_original_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mut svc = service(dir.path()).await;
    let mut command = spec(&svc);
    command.options.execution = ExecutionSelection::User {
        context_ref: ExecutionContextRef::new(),
    };
    let id = RequestId::new();
    assert!(matches!(
        svc.submit_command(actor(), id, command.clone()).await,
        Err(TaskServiceError::ExecutionContext(_))
    ));
    assert!(svc.store.incomplete_task_refs().await.unwrap().is_empty());
    let original = svc.execution_context.clone();
    let accepted = svc
        .store
        .accept_command(
            svc.device_ref,
            actor(),
            id,
            &command,
            original,
            unix_millis().unwrap(),
        )
        .await
        .unwrap();
    let AcceptTaskOutcome::Created(snapshot) = accepted else {
        panic!("unexpected replay")
    };
    svc.execution_context
        .environment_revision
        .push_str("-changed");
    assert_eq!(
        svc.submit_command(actor(), id, command.clone())
            .await
            .unwrap(),
        snapshot
    );
    command.options.execution = ExecutionSelection::Service {};
    assert!(matches!(
        svc.submit_command(actor(), id, command).await,
        Err(TaskServiceError::Store(TaskStoreError::RequestConflict))
    ));
}
#[tokio::test]
#[ignore = "requires PAB_EXECUTION_TEST_WORKER and PAB_EXECUTION_TEST_USER; runs real user processes"]
async fn native_user_command_acceptance() {
    use pab_os_control::execution::PreparedUser;
    use pab_protocol::{ExecutionMode, SystemQuery, SystemQueryData};
    let selected: u32 = std::env::var("PAB_EXECUTION_TEST_USER")
        .unwrap()
        .parse()
        .unwrap();
    #[cfg(windows)]
    let prepared = PreparedUser::for_session(selected).unwrap();
    #[cfg(unix)]
    let prepared = PreparedUser::for_uid(selected).unwrap();
    let expected = prepared.identity().clone();
    let dir = tempfile::tempdir().unwrap();
    let mut svc = service(dir.path()).await.for_ui_connection();
    svc.worker_executable = Some(
        std::env::var_os("PAB_EXECUTION_TEST_WORKER")
            .unwrap()
            .into(),
    );
    let id = RequestId::new();
    let mut query = svc
        .system_query(
            actor(),
            id,
            SystemQuery::ExecutionContexts {
                user: Some(expected.account_name.clone()),
                include_system: true,
                limit: 100,
            },
        )
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while query.state == "running" {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
        query = svc.get_system_query(actor(), id).await.unwrap();
    }
    let Some(SystemQueryData::ExecutionContexts { entries, .. }) = query.data else {
        panic!("missing discovery")
    };
    let selection = entries
        .iter()
        .find(|e| {
            e.mode == ExecutionMode::User
                && e.account_id.as_ref() == Some(&expected.account_id)
                && e.session_id == expected.session_id.map(|v| v.to_string())
        })
        .unwrap()
        .selection
        .unwrap();
    let mut command = spec(&svc);
    command.options.execution = selection;
    command
        .options
        .env
        .insert("PAB_WORKER_MARKER".into(), "用户环境".into());
    command.options.stdin_text = Some("输入验证".into());
    #[cfg(windows)]
    {
        command.program = "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".into();
        command.args=vec!["-NoProfile".into(),"-NonInteractive".into(),"-Command".into(),"[Console]::InputEncoding=[Text.UTF8Encoding]::new($false); [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); whoami /user; [Console]::Out.Write($env:PAB_WORKER_MARKER); [Console]::Out.Write([Console]::In.ReadToEnd())".into()];
    }
    #[cfg(unix)]
    {
        command.program = "/bin/sh".into();
        command.args = vec![
            "-c".into(),
            "id; printf '%s' \"$PAB_WORKER_MARKER\"; cat".into(),
        ];
    }
    let request = RequestId::new();
    let task = svc
        .submit_command(actor(), request, command.clone())
        .await
        .unwrap();
    let result = finished(&svc, task.task_ref).await;
    assert_eq!(result.state, TaskState::Succeeded, "{result:?}");
    let actual = result.execution_context.identity.as_ref().unwrap();
    assert_eq!(actual.account_id, expected.account_id);
    assert_eq!(actual.mode, ExecutionMode::User);
    assert_eq!(
        result.execution_context.cwd.as_deref(),
        expected.home.to_str()
    );
    let output = svc
        .store
        .read_output(actor(), task.task_ref, OutputStream::Stdout, 0, 8192)
        .await
        .unwrap();
    let text = String::from_utf8(output.0.bytes).unwrap();
    let marker = if cfg!(windows) {
        expected.account_id.clone()
    } else {
        format!("uid={}", expected.account_id.strip_prefix("uid:").unwrap())
    };
    assert!(text.contains(&marker), "{text}");
    assert!(text.contains("用户环境输入验证"), "{text}");
    assert_eq!(
        svc.submit_command(actor(), request, command.clone())
            .await
            .unwrap(),
        result
    );
    command.options.execution = ExecutionSelection::Service {};
    assert!(
        svc.submit_command(actor(), request, command.clone())
            .await
            .is_err()
    );
    command.options.execution = selection;
    command.options.stdin_text = None;
    #[cfg(windows)]
    {
        command.args = vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            "Start-Sleep -Seconds 30".into(),
        ];
    }
    #[cfg(unix)]
    {
        command.program = "/bin/sleep".into();
        command.args = vec!["30".into()];
    }
    command.options.timeout_ms = Some(100);
    let task = svc
        .submit_command(actor(), RequestId::new(), command.clone())
        .await
        .unwrap();
    let stopped = finished(&svc, task.task_ref).await;
    assert_eq!(stopped.state, TaskState::Failed, "{stopped:?}");
    assert_eq!(stopped.error.unwrap().code, "execution_timeout");
    command.options.timeout_ms = None;
    let task = svc
        .submit_command(actor(), RequestId::new(), command)
        .await
        .unwrap();
    svc.cancel(actor(), task.task_ref, "fixture cancellation".into())
        .await
        .unwrap();
    let stopped = finished(&svc, task.task_ref).await;
    assert_eq!(stopped.state, TaskState::Cancelled, "{stopped:?}");
}
