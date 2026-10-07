async fn application_helper() -> (tempfile::TempDir, LocalSocket, tokio::task::JoinHandle<Result<(), LocalIpcError>>, u64, pab_protocol::ExecutionIdentity) {
    application_helper_mode(false).await
}
async fn application_helper_mode(only: bool) -> (tempfile::TempDir, LocalSocket, tokio::task::JoinHandle<Result<(), LocalIpcError>>, u64, pab_protocol::ExecutionIdentity) {
    let directory = tempfile::tempdir().unwrap();
    let token = ensure_machine_token(directory.path()).unwrap();
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(serve(listener, directory.path().to_path_buf(), token));
    let mut socket = connect_with_token(port, &token).await.unwrap();
    if only {
        register_application_only_helper(&mut socket).await.unwrap();
    } else {
        register_application_helper(&mut socket).await.unwrap();
    }
    let (id, identity) = {
        let providers = window_providers().lock().unwrap();
        let provider = providers.last().unwrap();
        (provider.id, provider.identity.as_ref().expect("run this test in a native desktop user session").identity.clone())
    };
    (directory, socket, server, id, identity)
}

#[tokio::test]
async fn application_only_registration_preserves_window_route_and_release_broadcast() {
    let _guard = TEST_HELPER_LOCK.lock().await;
    let (_win_dir, mut windows, window_server, window_id) = screenshot_helper().await;
    let (_app_dir, mut apps, app_server, app_id, identity) = application_helper_mode(true).await;
    assert!(applications::ApplicationRoute::select(&identity, 1).is_ok());
    assert!(applications::ApplicationRoute::select(&identity, 2).is_ok());
    {
        let mut providers = window_providers().lock().unwrap();
        providers.iter_mut().find(|p| p.id == app_id).unwrap().application_schema_version = Some(1);
    }
    assert!(applications::ApplicationRoute::select(&identity, 2).is_err());
    assert!(applications::ApplicationRoute::select(&identity, 1).is_ok());
    let request = tokio::spawn(request_window_list());
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match next_local_event(&mut windows).await.unwrap() {
                LocalEvent::ListWindows => break,
                LocalEvent::Status(_) | LocalEvent::StatusUnavailable(_) => {},
                other => panic!("unexpected window event {other:?}"),
            }
        }
    }).await.unwrap();
    reply_window_list(&mut windows, &[]).await.unwrap();
    assert!(request.await.unwrap().unwrap().is_empty());
    let connection = pab_protocol::RequestId::new();
    release_ui_connection(connection).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let LocalEvent::ReleaseUiConnection(id) = next_local_event(&mut windows).await.unwrap() {
                assert_eq!(id, connection); break;
            }
        }
    }).await.unwrap();
    windows.close(None).await.unwrap(); wait_helper_removed(window_id).await; window_server.abort();
    // With only the app channel remaining, all legacy requests fail before
    // dispatch instead of using the wrong session/helper.
    assert!(matches!(request_window_list().await, Err(LocalIpcError::WindowHelperUnavailable)));
    assert!(matches!(request_screenshot().await, Err(LocalIpcError::WindowHelperUnavailable)));
    assert!(matches!(request_screenshot_v2(pab_protocol::ScreenshotOptions::default()).await, Err(LocalIpcError::WindowHelperUnavailable)));
    assert!(matches!(request_desktop_input(DesktopInputEvent::Key { virtual_key: 0x41, down: false }).await, Err(LocalIpcError::WindowHelperUnavailable)));
    assert!(matches!(request_desktop_query(pab_protocol::RequestId::new(), pab_protocol::DesktopQuery::Windows {}).await, Err(LocalIpcError::WindowHelperUnavailable)));
    let unexpected = tokio::time::timeout(Duration::from_millis(150), async {
        loop {
            match next_local_event(&mut apps).await.unwrap() {
                LocalEvent::Status(_) | LocalEvent::StatusUnavailable(_) => {},
                other => return other,
            }
        }
    }).await;
    assert!(unexpected.is_err(), "app-only helper received a window/input request");
    apps.close(None).await.unwrap(); wait_helper_removed(app_id).await; app_server.abort();
}
async fn next_application(socket: &mut LocalSocket) -> (pab_protocol::RequestId, pab_protocol::AppQuery, pab_protocol::ExecutionIdentity) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            match next_local_event(socket).await.unwrap() {
                LocalEvent::ApplicationQuery(id, query, identity) => return (id, query, identity),
                LocalEvent::Status(_) | LocalEvent::StatusUnavailable(_) => {},
                other => panic!("unexpected application event: {other:?}"),
            }
        }
    }).await.unwrap()
}
async fn application_selection(svc: &crate::task_service::TaskService, identity: &pab_protocol::ExecutionIdentity) -> pab_protocol::ExecutionSelection {
    use crate::task_service::transfer_tests::actor;
    use pab_protocol::*;
    let id = RequestId::new();
    let mut reply = svc.system_query(actor(), id, SystemQuery::ExecutionContexts { user: Some(identity.account_name.clone()), include_system: false, limit: 1000 }).await.unwrap();
    for _ in 0..200 {
        if reply.state != "running" { break; }
        tokio::time::sleep(Duration::from_millis(20)).await;
        reply = svc.get_system_query(actor(), id).await.unwrap();
    }
    assert_eq!(reply.state, "completed", "{:?}", reply.error);
    let Some(SystemQueryData::ExecutionContexts { entries, .. }) = reply.data else { panic!("missing inventory") };
    let entry = entries.iter().find(|entry| entry.identity.as_ref() == Some(identity)).expect("verified desktop session selectable");
    entry.validate().unwrap();
    entry.selection.unwrap()
}

#[tokio::test]
async fn application_routes_bind_native_identity_and_durable_duplicates() {
    use crate::task_service::transfer_tests::{actor, service};
    use pab_protocol::*;
    let _guard = TEST_HELPER_LOCK.lock().await;
    let (directory, mut socket, local, provider, identity) = application_helper().await;
    assert_eq!(identity.mode, ExecutionMode::DesktopUser);
    let mut different = identity.clone();
    different.session_id = Some("other-session".into());
    assert!(applications::ApplicationRoute::select(&different, 1).is_err());
    different = identity.clone(); different.account_id.push_str("-other");
    assert!(applications::ApplicationRoute::select(&different, 1).is_err());
    let svc = service(directory.path()).await.for_ui_connection();
    let selection = application_selection(&svc, &identity).await;
    let other = svc.for_ui_connection();
    let request = AppQuery::Execute { request: AppActionRequest::Launch { new_instance: false, application: AppTarget::Id { id: "fixture.never-launched".into() } } };
    let query = SystemQuery::Applications { execution: selection, query: request.clone() };
    assert!(other.system_query(actor(), RequestId::new(), query.clone()).await.is_err());
    let id = RequestId::new();
    let pending = svc.system_query(actor(), id, query.clone()).await.unwrap();
    assert_eq!(pending.state, "running");
    assert_eq!(pending.execution_context.as_ref().unwrap().identity.as_ref(), Some(&identity));
    let (seen, app, expected) = next_application(&mut socket).await;
    assert_eq!((seen, app, expected), (id, request, identity.clone()));
    assert_eq!(svc.system_query(actor(), id, query.clone()).await.unwrap().state, "running");
    let mut result = SystemQueryReply::pending(id, &query);
    result.state = "completed".into();
    result.data = Some(SystemQueryData::Applications { snapshot: AppSnapshot::Action { result: AppActionResult {
        request_accepted: true, instance: None, reused_instance: None, execution_identity: identity, window_ready: None, notes: vec!["protocol fixture; no native launch".into()],
    } } });
    reply_desktop_query(&mut socket, &result).await.unwrap();
    for _ in 0..100 {
        let r = svc.get_system_query(actor(), id).await.unwrap();
        if r.state == "completed" { break; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let result = svc.get_system_query(actor(), id).await.unwrap();
    assert_eq!(result.state, "completed");
    socket.close(None).await.unwrap(); wait_helper_removed(provider).await; local.abort();
    // Re-read persists without a live helper or a valid reference on this connection.
    let restarted = service(directory.path()).await.for_ui_connection();
    assert_eq!(restarted.system_query(actor(), id, query.clone()).await.unwrap(), result);
    let mut changed = query.clone();
    if let SystemQuery::Applications { query: AppQuery::Execute { request: AppActionRequest::Launch { new_instance: false, application: AppTarget::Id { id } } }, .. } = &mut changed { id.push_str("changed"); }
    assert!(svc.system_query(actor(), id, changed).await.is_err());
}

#[tokio::test]
async fn application_disconnect_is_unconfirmed_and_never_replayed_to_replacement() {
    use crate::task_service::transfer_tests::{actor, service};
    use pab_protocol::*;
    let _guard = TEST_HELPER_LOCK.lock().await;
    let (directory, mut socket, local, provider, identity) = application_helper().await;
    let frozen = applications::ApplicationRoute::select(&identity, 1).unwrap();
    let svc = service(directory.path()).await;
    let selection = application_selection(&svc, &identity).await;
    let app = AppQuery::Execute { request: AppActionRequest::Launch { new_instance: false, application: AppTarget::Id { id: "fixture.never-launched".into() } } };
    let query = SystemQuery::Applications { execution: selection, query: app.clone() };
    let id = RequestId::new();
    assert_eq!(svc.system_query(actor(), id, query.clone()).await.unwrap().state, "running");
    assert_eq!(next_application(&mut socket).await.0, id);
    socket.close(None).await.unwrap(); wait_helper_removed(provider).await; local.abort();
    for _ in 0..100 {
        if svc.get_system_query(actor(), id).await.unwrap().state != "running" { break; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(svc.get_system_query(actor(), id).await.unwrap().state, "unconfirmed");
    let (_dir2, mut replacement, local2, provider2, _) = application_helper().await;
    assert!(matches!(frozen.execute(RequestId::new(), app).await, Err(LocalIpcError::WindowHelperUnavailable)));
    let original = svc.system_query(actor(), id, query.clone()).await.unwrap();
    assert_eq!(original.state, "unconfirmed");
    assert_eq!(original.execution_context.as_ref().unwrap().identity.as_ref(), Some(&identity));
    // Reopening the durable store and using a new UI connection must observe
    // the same uncertain mutation, never dispatch it to the replacement.
    let restarted = service(directory.path()).await.for_ui_connection();
    assert_eq!(restarted.system_query(actor(), id, query).await.unwrap(), original);
    let unexpected = tokio::time::timeout(Duration::from_millis(150), async {
        loop { if matches!(next_local_event(&mut replacement).await.unwrap(), LocalEvent::ApplicationQuery(..)) { return; } }
    }).await;
    assert!(unexpected.is_err(), "original operation must never reach replacement helper");
    replacement.close(None).await.unwrap(); wait_helper_removed(provider2).await; local2.abort();
}

#[tokio::test]
async fn application_reply_rejects_wrong_account_and_wrong_payload_kind() {
    use pab_protocol::*;
    let _guard = TEST_HELPER_LOCK.lock().await;
    let (_directory, mut socket, local, provider, identity) = application_helper().await;
    for wrong_account in [true, false] {
        let route = applications::ApplicationRoute::select(&identity, 1).unwrap();
        let id = RequestId::new();
        let query = AppQuery::List { request: AppListRequest { scope: AppListScope::Running, search: String::new(), limit: 10 } };
        let work = tokio::spawn(route.execute(id, query.clone()));
        assert_eq!(next_application(&mut socket).await.0, id);
        let mut reply = SystemQueryReply::pending_kind(id, query.kind());
        reply.state = "completed".into();
        let mut observed = identity.clone();
        if wrong_account { observed.account_id.push_str("-other"); }
        reply.data = Some(SystemQueryData::Applications { snapshot: if wrong_account {
            AppSnapshot::List { snapshot: AppListSnapshot { apps: vec![], truncated: false, sources: vec![], warnings: vec![], execution_identity: observed } }
        } else {
            AppSnapshot::Action { result: AppActionResult { request_accepted: true, instance: None, reused_instance: None, execution_identity: observed, window_ready: None, notes: vec![] } }
        } });
        reply_desktop_query(&mut socket, &reply).await.unwrap();
        assert!(matches!(work.await.unwrap(), Err(LocalIpcError::Protocol)));
    }
    socket.close(None).await.unwrap(); wait_helper_removed(provider).await; local.abort();
}
