#[tokio::test]
async fn ui_waits_release_slots_connections_are_distinct_and_action_deduplicates() {
    use crate::task_service::transfer_tests::{actor, service};
    use pab_protocol::*;
    let _guard = TEST_HELPER_LOCK.lock().await;
    let (directory, mut socket, local, provider) = screenshot_helper().await;
    let svc = service(directory.path()).await;
    let first = svc.for_ui_connection();
    let second = svc.for_ui_connection();
    let (events, mut seen) = tokio::sync::mpsc::unbounded_channel();
    let helper = tokio::spawn(async move {
        loop {
            match next_local_event(&mut socket).await.unwrap() {
                LocalEvent::DesktopQuery(id, query, context) => {
                    let context = context.expect("server-generated UI context");
                    let mutation = query.is_mutation();
                    events
                        .send((id, context.connection_id, context.sample, mutation))
                        .unwrap();
                    let mut reply = SystemQueryReply::pending(id, &SystemQuery::Desktop { query });
                    reply.state = "completed".into();
                    let mut snapshot = DesktopSnapshot::new("fixture".into(), "test");
                    snapshot.ui = Some(UiSnapshot {
                        elements: vec![],
                        visited_count: 0,
                        truncated: false,
                        stop_reason: None,
                        outcome: UiOutcome::Completed,
                        action_dispatched: Some(mutation),
                        verification: UiVerification::NativeReturned,
                        error_code: None,
                        sampled_from_unix_ms: 0,
                        sampled_at_unix_ms: 0,
                    });
                    reply.data = Some(SystemQueryData::Desktop { snapshot });
                    reply_desktop_query(&mut socket, &reply).await.unwrap();
                }
                LocalEvent::Status(_)
                | LocalEvent::StatusUnavailable(_)
                | LocalEvent::ReleaseUiConnection(_) => {}
                other => panic!("unexpected helper request: {other:?}"),
            }
        }
    });
    let wait = SystemQuery::Desktop {
        query: DesktopQuery::Ui {
            query: UiRequest::Wait {
                scope: UiScope::Window {
                    window_ref: RequestId::new().to_string(),
                },
                selector: UiSelector::default(),
                condition: UiCondition::Exists,
                timeout_ms: 10_000,
                poll_ms: 100,
            },
        },
    };
    let a = RequestId::new();
    let b = RequestId::new();
    assert_eq!(
        first
            .system_query(actor(), a, wait.clone())
            .await
            .unwrap()
            .state,
        "running"
    );
    assert_eq!(
        second
            .system_query(actor(), b, wait.clone())
            .await
            .unwrap()
            .state,
        "running"
    );
    let action = SystemQuery::Desktop {
        query: DesktopQuery::Ui {
            query: UiRequest::Action {
                element_ref: RequestId::new().to_string(),
                action: UiAction::Invoke,
                expected: UiExpected::default(),
                timeout_ms: 1000,
            },
        },
    };
    let action_id = RequestId::new();
    let actual = tokio::time::timeout(
        Duration::from_secs(2),
        first.system_query(actor(), action_id, action.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        actual.state, "completed",
        "two waiting queries must not starve the action"
    );
    let duplicate = first
        .system_query(actor(), action_id, action)
        .await
        .unwrap();
    assert_eq!(duplicate, actual);
    for (s, id) in [(&first, a), (&second, b)] {
        assert_eq!(
            s.cancel_system_query(actor(), id).await.unwrap().state,
            "cancel_requested"
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if s.get_system_query(actor(), id).await.unwrap().state == "cancelled" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    let mut first_owner = None;
    let mut second_owner = None;
    let mut calls = 0;
    while let Ok((id, owner, sample, mutation)) = seen.try_recv() {
        if id == a {
            assert!(sample);
            first_owner = Some(owner);
        }
        if id == b {
            assert!(sample);
            second_owner = Some(owner);
        }
        if id == action_id {
            assert!(!sample && mutation);
            calls += 1;
            assert_eq!(Some(owner), first_owner);
        }
    }
    assert!(first_owner.is_some() && second_owner.is_some());
    assert_ne!(first_owner, second_owner);
    assert_eq!(calls, 1);
    helper.abort();
    local.abort();
    let _ = helper.await;
    let _ = local.await;
    wait_helper_removed(provider).await;
}
