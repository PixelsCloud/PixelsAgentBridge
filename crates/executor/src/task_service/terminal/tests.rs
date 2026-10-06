use super::*;
use crate::task_service::transfer_tests::{actor, service};
use pab_protocol::{ExecutionMode, ExecutionSelection, SystemQuery, SystemQueryData};

#[tokio::test]
async fn unknown_user_terminal_does_not_create_a_service_session() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await.for_ui_connection();
    assert!(matches!(
        svc.open_terminal_session(
            actor(),
            RequestId::new(),
            80,
            24,
            ExecutionSelection::User {
                context_ref: pab_protocol::ExecutionContextRef::new()
            }
        )
        .await,
        Err(TaskServiceError::ExecutionContext(_))
    ));
    assert!(svc.terminals.lock().await.is_empty());
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite:{}",
        dir.path().join("executor.sqlite3").display()
    ))
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM terminal_sessions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn terminal_lost_result_is_persisted_as_unconfirmed_not_replayed() {
    use sqlx::Row;
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await.for_ui_connection();
    let id = RequestId::new();
    svc.store
        .start_terminal(
            id,
            actor(),
            "fixture",
            svc.ui_connection.id,
            Default::default(),
            None,
            80,
            24,
        )
        .await
        .unwrap();
    svc.store
        .begin_terminal_event(id, 1, "input", b"fixture")
        .await
        .unwrap();
    assert!(
        svc.finish_terminal_action(id, 1, Err(TaskServiceError::InvalidRequest("lost reply")))
            .await
            .is_err()
    );
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite:{}",
        dir.path().join("executor.sqlite3").display()
    ))
    .await
    .unwrap();
    let row=sqlx::query("SELECT t.state AS task_state,e.state AS event_state FROM terminal_sessions t JOIN terminal_events e ON t.id=e.session_id WHERE t.id=?").bind(id.to_string()).fetch_one(&pool).await.unwrap();
    assert_eq!(row.get::<String, _>("task_state"), "interrupted");
    assert_eq!(row.get::<String, _>("event_state"), "unconfirmed");
    assert!(
        svc.store
            .begin_terminal_event(id, 2, "input", b"fixture")
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires PAB_EXECUTION_TEST_WORKER and PAB_EXECUTION_TEST_USER; creates real user PTYs"]
async fn native_user_terminal_acceptance() {
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
    let other = base.for_ui_connection();
    svc.worker_executable = Some(
        std::env::var_os("PAB_EXECUTION_TEST_WORKER")
            .unwrap()
            .into(),
    );
    let guard = svc.terminal_connection_guard();
    let request = RequestId::new();
    let mut query = svc
        .system_query(
            actor(),
            request,
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
        query = svc.get_system_query(actor(), request).await.unwrap();
    }
    let Some(SystemQueryData::ExecutionContexts { entries, .. }) = query.data else {
        panic!("no discovery")
    };
    let selection = entries
        .iter()
        .find(|e| {
            e.mode == ExecutionMode::User
                && e.account_id.as_ref() == Some(&expected.account_id)
                && e.session_id == expected.session_id.map(|n| n.to_string())
        })
        .unwrap()
        .selection
        .unwrap();
    let id = RequestId::new();
    let entry = svc
        .open_terminal_session(actor(), id, 80, 24, selection)
        .await
        .unwrap();
    assert_eq!(
        entry.identity.as_ref().unwrap().account_id,
        expected.account_id
    );
    assert!(Arc::ptr_eq(
        &entry,
        &svc.open_terminal_session(actor(), id, 80, 24, selection)
            .await
            .unwrap()
    ));
    assert!(matches!(
        svc.open_terminal_session(actor(), id, 80, 24, Default::default())
            .await,
        Err(TaskServiceError::Store(
            crate::task_store::TaskStoreError::RequestConflict
        ))
    ));
    assert!(matches!(
        other.terminal_entry(id, actor()).await,
        Err(TaskServiceError::AccessDenied)
    ));
    assert!(matches!(
        other
            .open_terminal_session(actor(), RequestId::new(), 80, 24, selection)
            .await,
        Err(TaskServiceError::ExecutionContext(_))
    ));
    entry.session.resize(100, 32).await.unwrap();
    #[cfg(windows)]
    let command = "whoami /user; Write-Output ([string][char]0x4E2D+[char]0x6587); (Get-Location).Path; (Get-CimInstance Win32_Process -Filter \"ProcessId=$PID\").CommandLine\r";
    #[cfg(unix)]
    let command = "id; printf '\\344\\270\\255\\346\\226\\207\\n'; pwd; ps -p $$ -o args=\n";
    entry.session.input(command.as_bytes()).await.unwrap();
    let marker = if cfg!(windows) {
        expected.account_id.clone()
    } else {
        format!("uid={}", expected.account_id.strip_prefix("uid:").unwrap())
    };
    let until = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = entry.session.read(offset, 8192).await.unwrap();
        offset = chunk.next_offset;
        bytes.extend(chunk.bytes);
        let text = String::from_utf8_lossy(&bytes);
        if text.contains(&marker)
            && text.contains("中文")
            && entry
                .startup
                .arguments
                .iter()
                .all(|argument| text.contains(argument))
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < until,
            "missing identity/Unicode/actual shell arguments: {text}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite:{}",
        dir.path().join("executor.sqlite3").display()
    ))
    .await
    .unwrap();
    let persisted: String =
        sqlx::query_scalar("SELECT identity_json FROM terminal_execution WHERE session_id=?")
            .bind(id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        serde_json::from_str::<pab_protocol::ExecutionIdentity>(&persisted).unwrap(),
        entry.identity.clone().unwrap()
    );
    entry.session.close().await.unwrap();
    entry.session.close().await.unwrap();
    assert!(entry.session.input(b"never execute\n").await.is_err());
    // A second PTY is closed by connection teardown, with a durable result.
    let id2 = RequestId::new();
    let second = svc
        .open_terminal_session(actor(), id2, 80, 24, selection)
        .await
        .unwrap();
    drop(guard);
    let until = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let state: String = sqlx::query_scalar("SELECT state FROM terminal_sessions WHERE id=?")
            .bind(id2.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
        if state == "interrupted" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < until,
            "connection cleanup did not finish"
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(second.session.input(b"never execute\n").await.is_err());
    assert!(
        svc.open_terminal_session(actor(), RequestId::new(), 80, 24, selection)
            .await
            .is_err()
    );
}
