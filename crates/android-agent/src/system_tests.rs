use super::*;

fn execution() -> ExecutionContext {
    crate::peer::context(&crate::peer::RuntimeOptions {
        database_file: "/tmp/mobile.sqlite".into(),
        password_hash: String::new(),
        password_version: 1,
        android_release: "9".into(),
        android_sdk: 28,
        storage_staging_directory: None,
    })
    .unwrap()
}
fn launch(service: &SystemService, owner: &str, scope: &str) -> SystemQuery {
    SystemQuery::Applications {
        execution: ExecutionSelection::DesktopUser {
            context_ref: service.context_ref(&format!("{owner}:{scope}")),
        },
        query: AppQuery::Execute {
            request: AppActionRequest::Launch {
                application: AppTarget::Id {
                    id: "com.pixels.fixture".into(),
                },
                new_instance: false,
            },
        },
    }
}
async fn job(service: &SystemService) -> Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let value = service.sdk.poll().await;
            if !value.is_null() {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
async fn complete(service: &SystemService, job: Value, value: Value) {
    assert_eq!(job["command"]["domain"], "system");
    assert!(
        service
            .sdk
            .complete(
                serde_json::from_value(job["ticket"].clone()).unwrap(),
                value
            )
            .await
    );
}
fn accepted() -> Value {
    json!({"count":1,"warnings":[],"data":{"type":"applications","snapshot":{"type":"action","result":{
    "request_accepted":true,"instance":null,"reused_instance":null,"execution_identity":{
        "mode":"desktop_user","account_id":"uid:10001","account_name":"com.pixels.fixture","home":"/workspace",
        "primary_group":10001,"session_id":"android-user:0:display:0","logon_id":null,"environment_source":"interactive_session"},
    "window_ready":null,"notes":[]}}}})
}

#[tokio::test]
async fn notification_contents_are_transient_and_sdk_references_have_connection_scope() {
    let temp = tempfile::tempdir().unwrap();
    let service = SystemService::open(
        &temp.path().join("notifications.sqlite"),
        [19; 32],
        DeviceRef {
            tenant_id: TenantId::new(),
            device_id: DeviceId::new(),
        },
    )
    .await
    .unwrap();
    let id = RequestId::new();
    let worker = service.clone();
    let task = tokio::spawn(async move {
        worker
            .request(
                "owner",
                "connection-a",
                id,
                SystemQuery::Notifications {
                    query: NotificationRequest::List {
                        package_id: "com.pixels.fixture".into(),
                        limit: 16,
                    },
                },
                execution(),
            )
            .await
    });
    let work = job(&service).await;
    assert_eq!(work["command"]["connection_scope"], "connection-a");
    let caller = work["command"]["caller"].clone();
    assert_eq!(caller.as_str().unwrap().len(), 64);
    complete(&service,work,json!({"count":1,"warnings":[],"data":{"type":"notifications","snapshot":{"access_granted":true,"listener_connected":true,"revision":1,"entries":[{"notification_ref":RequestId::new(),"package_id":"com.pixels.fixture","title":"private-notification-text","text":"private-body","posted_at_unix_ms":1,"ongoing":false,"clearable":true,"can_open":true}],"outcome":"sampled","action_dispatched":false,"truncated":false}}})).await;
    let reply = task.await.unwrap();
    assert_eq!(reply.state, "completed");
    let stored: String = sqlx::query_scalar("SELECT reply FROM system_operations WHERE id=?")
        .bind(id.to_string())
        .fetch_one(&service.pool)
        .await
        .unwrap();
    assert!(!stored.contains("private-notification-text") && !stored.contains("private-body"));
    assert_eq!(
        service.lookup("foreign-owner", id).await.error.as_deref(),
        Some("not_found")
    );
    let worker = service.clone();
    let reference = RequestId::new().to_string();
    let action = tokio::spawn(async move {
        worker
            .request(
                "owner",
                "connection-b",
                RequestId::new(),
                SystemQuery::Notifications {
                    query: NotificationRequest::Open {
                        notification_ref: reference,
                    },
                },
                execution(),
            )
            .await
    });
    let work = job(&service).await;
    assert_eq!(work["command"]["caller"], caller);
    assert_eq!(work["command"]["connection_scope"], "connection-b");
    complete(
        &service,
        work,
        json!({"error":"notification_reference_expired"}),
    )
    .await;
    assert_eq!(
        action.await.unwrap().error.as_deref(),
        Some("notification_reference_expired")
    );
    let permit = service.capture_permit().unwrap();
    assert!(service.capture_permit().is_err());
    drop(permit);
    assert!(service.capture_permit().is_ok());
}

#[tokio::test]
async fn package_dispatch_and_async_status_are_separate_and_not_replayed() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("package-system.sqlite");
    let device = DeviceRef {
        tenant_id: TenantId::new(),
        device_id: DeviceId::new(),
    };
    let service = SystemService::open(&path, [7; 32], device).await.unwrap();
    let id = RequestId::new();
    let query = SystemQuery::Applications {
        execution: ExecutionSelection::DesktopUser {
            context_ref: service.context_ref("owner:session"),
        },
        query: AppQuery::Package {
            request: AppPackageRequest::Uninstall {
                package_id: "com.pixels.fixture".into(),
                version_code: 1,
            },
        },
    };
    let worker = service.clone();
    let original = query.clone();
    let pending = tokio::spawn(async move {
        worker
            .request("owner", "session", id, original, execution())
            .await
    });
    let work = job(&service).await;
    assert_eq!(work["command"]["request_id"], json!(id));
    assert_eq!(work["command"]["caller"].as_str().unwrap().len(), 64);
    let identity = accepted()["data"]["snapshot"]["result"]["execution_identity"].clone();
    complete(&service, work, json!({"count":1,"data":{"type":"applications","snapshot":{"type":"package","result":{
        "change_ref":id,"state":"waiting_user_action","package":null,"package_id":"com.pixels.fixture","error":null,
        "can_request_installs":false,"execution_identity":identity,"notes":[]}}}})).await;
    let reply = pending.await.unwrap();
    assert_eq!(reply.state, "completed");
    assert!(matches!(
        reply.data,
        Some(SystemQueryData::Applications {
            snapshot: AppSnapshot::Package {
                result: AppPackageResult {
                    state: AppPackageState::WaitingUserAction,
                    ..
                }
            }
        })
    ));
    service.pool.close().await;
    drop(service);
    let restarted = SystemService::open(&path, [7; 32], device).await.unwrap();
    assert_eq!(
        restarted
            .request("owner", "different-session", id, query, execution())
            .await
            .state,
        "completed"
    );
    assert!(restarted.sdk.poll().await.is_null());
    assert_eq!(
        restarted
            .lookup("different-owner", id)
            .await
            .error
            .as_deref(),
        Some("not_found")
    );
}

#[tokio::test]
async fn accepted_activity_is_not_replayed_after_reconnect_restart_or_parameter_conflict() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("system.sqlite");
    let device = DeviceRef {
        tenant_id: TenantId::new(),
        device_id: DeviceId::new(),
    };
    let service = SystemService::open(&path, [7; 32], device).await.unwrap();
    let id = RequestId::new();
    let query = launch(&service, "owner", "session");
    let worker = service.clone();
    let original = query.clone();
    let pending = tokio::spawn(async move {
        worker
            .request("owner", "session", id, original, execution())
            .await
    });
    let work = job(&service).await;
    let row: String = sqlx::query_scalar("SELECT reply FROM system_operations WHERE id=?")
        .bind(id.to_string())
        .fetch_one(&service.pool)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<SystemQueryReply>(&row)
            .unwrap()
            .state,
        "running"
    );
    complete(&service, work, accepted()).await;
    assert_eq!(pending.await.unwrap().state, "completed");
    assert_eq!(
        service
            .request("owner", "new-session", id, query.clone(), execution())
            .await
            .state,
        "completed"
    );
    let wrong = service
        .request(
            "owner",
            "new-session",
            RequestId::new(),
            query.clone(),
            execution(),
        )
        .await;
    assert_eq!(
        wrong.error.as_deref(),
        Some("execution_context_expired_or_wrong_caller")
    );
    assert_eq!(
        service.lookup("other-owner", id).await.error.as_deref(),
        Some("not_found")
    );
    let conflict = service
        .request(
            "owner",
            "session",
            id,
            SystemQuery::Disks { limit: 2 },
            execution(),
        )
        .await;
    assert_eq!(conflict.error.as_deref(), Some("request_conflict"));
    assert!(service.sdk.poll().await.is_null());
    service.pool.close().await;
    drop(service);
    let restarted = SystemService::open(&path, [7; 32], device).await.unwrap();
    assert_eq!(
        restarted
            .request("owner", "restarted", id, query, execution())
            .await
            .state,
        "completed"
    );
    assert!(restarted.sdk.poll().await.is_null());
}

#[tokio::test]
async fn read_results_are_transient_and_never_automatically_sampled_again() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("system.sqlite");
    let device = DeviceRef {
        tenant_id: TenantId::new(),
        device_id: DeviceId::new(),
    };
    let service = SystemService::open(&path, [7; 32], device).await.unwrap();
    let id = RequestId::new();
    let query = SystemQuery::Networks {
        name: None,
        limit: 2,
    };
    let worker = service.clone();
    let original = query.clone();
    let pending = tokio::spawn(async move {
        worker
            .request("owner", "scope", id, original, execution())
            .await
    });
    let work = job(&service).await;
    complete(&service,work,json!({"data":{"type":"networks","entries":[{"name":"secret-network-marker","addresses":[],"mac":null,"mtu":null,"state":"active","received_bytes":null,"transmitted_bytes":null}]},"count":1,"warnings":[]})).await;
    assert_eq!(pending.await.unwrap().state, "completed");
    let row: String = sqlx::query_scalar("SELECT reply FROM system_operations WHERE id=?")
        .bind(id.to_string())
        .fetch_one(&service.pool)
        .await
        .unwrap();
    assert!(!row.contains("secret-network-marker"));
    service.pool.close().await;
    drop(service);
    let restarted = SystemService::open(&path, [7; 32], device).await.unwrap();
    assert_eq!(
        restarted
            .request("owner", "scope", id, query, execution())
            .await
            .error
            .as_deref(),
        Some("result_expired_original_not_replayed")
    );
    assert!(restarted.sdk.poll().await.is_null());
}

#[tokio::test]
async fn restart_marks_inflight_action_unconfirmed_and_journal_failure_is_not_not_found() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("system.sqlite");
    let device = DeviceRef {
        tenant_id: TenantId::new(),
        device_id: DeviceId::new(),
    };
    let service = SystemService::open(&path, [7; 32], device).await.unwrap();
    let id = RequestId::new();
    let query = launch(&service, "owner", "scope");
    let reply = SystemQueryReply::pending(id, &query);
    sqlx::query("INSERT INTO system_operations VALUES(?,?,?,?)")
        .bind("owner")
        .bind(id.to_string())
        .bind("fixture")
        .bind(serde_json::to_string(&reply).unwrap())
        .execute(&service.pool)
        .await
        .unwrap();
    service.pool.close().await;
    drop(service);
    let restarted = SystemService::open(&path, [7; 32], device).await.unwrap();
    assert_eq!(restarted.lookup("owner", id).await.state, "unconfirmed");
    assert!(restarted.sdk.poll().await.is_null());
    restarted.pool.close().await;
    assert_eq!(
        restarted
            .lookup("owner", RequestId::new())
            .await
            .error
            .as_deref(),
        Some("system_journal_unavailable")
    );
}
