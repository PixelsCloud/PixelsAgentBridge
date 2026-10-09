use super::*;
use pab_protocol::{
    AuthorizedDevicePeer, DeviceId, DeviceRef, EndpointUserContext, OperatorRef, UsageBatch,
    UsageCounters, UserAttribution, UserId,
};
use personal_devices::account;
use uuid::Uuid;

async fn saved_fixture(pool: &PgPool, user: &Value) -> AuthorizedDevicePeer {
    let endpoint = unclaimed_fixture(pool).await;
    let device_id: Uuid = sqlx::query_scalar("SELECT id FROM devices WHERE code=345678901")
        .fetch_one(pool)
        .await
        .unwrap();
    let user_id = UserId::from_uuid(user["id"].as_str().unwrap().parse().unwrap());
    AuthorizedDevicePeer {
        user_context: EndpointUserContext {
            revision: 1,
            policy_version: 1,
            user: Some(UserAttribution {
                user_id,
                username: user["username"].as_str().unwrap().into(),
            }),
        },
        device_ref: DeviceRef {
            tenant_id: endpoint.tenant_id,
            device_id: DeviceId::from_uuid(device_id),
        },
        peer_endpoint_key: endpoint.endpoint_key,
        operator: OperatorRef::guest(endpoint.endpoint_key),
        authorized_at_unix_ms: now(),
    }
}
fn now() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

#[sqlx::test(migrations = "./migrations")]
async fn thousand_saved_devices_page_with_changes_without_losing_tombstones(pool: PgPool) {
    let (app, _) = app(pool.clone(), true).await;
    let (token, user, _) = account(&app, "catalog-scale").await;
    let (_, _, other) = account(&app, "catalog-other").await;
    let tenant: Uuid = user["personal_tenant_id"].as_str().unwrap().parse().unwrap();
    let user_id: Uuid = user["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("INSERT INTO devices(id,tenant_id,code,name) SELECT gen_random_uuid(),$1,710000000+i,'Saved '||i FROM generate_series(1,1000) i").bind(tenant).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO user_catalog_versions(user_id,revision) VALUES($1,1000)").bind(user_id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO user_saved_devices(user_id,device_id,revision,saved_name,saved_system,verified_until) SELECT $1,id,code-710000000,name,'windows',to_timestamp(0) FROM devices").bind(user_id).execute(&pool).await.unwrap();
    let start = std::time::Instant::now();
    let mut cursor = 0; let mut seen = std::collections::HashSet::new(); let mut mutation = None;
    loop {
        let page = native_call(&app,"GET",&format!("/api/account/saved-devices?after={cursor}&limit=200"),&token,Value::Null).await;
        assert_eq!(page.0, StatusCode::OK);
        for item in page.1["items"].as_array().unwrap() {
            seen.insert(item["device_ref"]["device_id"].as_str().unwrap().to_owned());
            assert!(item["online"].is_null());
            if mutation.is_none() {
                mutation = Some(json!({"id":Uuid::new_v4(),"device_id":item["device_ref"]["device_id"],"expected_revision":item["revision"],"alias":"","deleted":true}));
            }
        }
        cursor = page.1["cursor"].as_i64().unwrap();
        if cursor == 200 {
            assert_eq!(native_call(&app,"POST","/api/account/saved-devices",&token,mutation.clone().unwrap()).await.0,StatusCode::OK);
        }
        if !page.1["has_more"].as_bool().unwrap() {break;}
    }
    assert_eq!(seen.len(),1000);
    assert_eq!(cursor,1001);
    assert_eq!(call(&app,"GET","/api/web/saved-devices",&other,Value::Null).await.2["items"],json!([]));
    eprintln!("1000 saved devices with concurrent deletion: {:?}", start.elapsed());
}

#[sqlx::test(migrations = "./migrations")]
async fn saved_devices_require_verified_target_and_isolate_mutations(pool: PgPool) {
    let (app, control) = app(pool.clone(), true).await;
    let (a, user, cookie) = account(&app, "saved-a").await;
    let (b, _, cookie_b) = account(&app, "saved-b").await;
    let peer = saved_fixture(&pool, &user).await;
    assert_eq!(
        native_call(&app, "GET", "/api/account/saved-devices", &a, Value::Null)
            .await
            .1["items"],
        json!([])
    );
    control
        .store()
        .record_authenticated_device(&peer)
        .await
        .unwrap();
    let list = native_call(&app, "GET", "/api/account/saved-devices", &a, Value::Null).await;
    assert_eq!(list.0, StatusCode::OK, "{}", list.1);
    assert_eq!(list.1["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/web/saved-devices",
            &cookie_b,
            Value::Null
        )
        .await
        .2["items"],
        json!([])
    );
    // Saving a remote device never grants ownership or full device management.
    assert_eq!(
        call(&app, "GET", "/api/web/devices", &cookie, Value::Null)
            .await
            .2["total"],
        0
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/web/devices/{}", peer.device_ref.device_id),
            &cookie,
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let mutation = json!({"id":Uuid::new_v4(),"device_id":peer.device_ref.device_id,"expected_revision":1,"alias":"my private alias","deleted":false});
    assert_eq!(
        native_call(
            &app,
            "POST",
            "/api/account/saved-devices",
            &b,
            mutation.clone()
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    for _ in 0..2 {
        let result = native_call(
            &app,
            "POST",
            "/api/account/saved-devices",
            &a,
            mutation.clone(),
        )
        .await;
        assert_eq!(result.0, StatusCode::OK, "{}", result.1);
        assert_eq!(result.1["revision"], 2);
    }
    let mut reused = mutation.clone();
    reused["alias"] = json!("changed request");
    assert_eq!(
        native_call(&app, "POST", "/api/account/saved-devices", &a, reused)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let remove = json!({"id":Uuid::new_v4(),"device_id":peer.device_ref.device_id,"expected_revision":2,"alias":"my private alias","deleted":true});
    assert_eq!(
        native_call(&app, "POST", "/api/account/saved-devices", &a, remove)
            .await
            .1["revision"],
        3
    );
    let mut stale = mutation;
    stale["id"] = json!(Uuid::new_v4());
    assert_eq!(
        native_call(&app, "POST", "/api/account/saved-devices", &a, stale)
            .await
            .0,
        StatusCode::CONFLICT
    );
    sqlx::query("UPDATE user_saved_devices SET verified_until=now()-interval '1 hour'")
        .execute(&pool)
        .await
        .unwrap();
    let restore = json!({"id":Uuid::new_v4(),"device_id":peer.device_ref.device_id,"expected_revision":3,"alias":"","deleted":false});
    assert_eq!(
        native_call(&app, "POST", "/api/account/saved-devices", &a, restore)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    control
        .store()
        .record_authenticated_device(&peer)
        .await
        .unwrap();
    let list = native_call(
        &app,
        "GET",
        "/api/account/saved-devices?after=2",
        &a,
        Value::Null,
    )
    .await
    .1;
    assert_eq!(list["items"][0]["deleted"], true);
    assert_eq!(list["items"][0]["alias"], "my private alias");
    assert!(list["items"][0]["online"].is_null());
}

#[sqlx::test(migrations = "./migrations")]
async fn usage_retries_are_atomic_and_personal_queries_cannot_read_other_accounts(pool: PgPool) {
    let (app, control) = app(pool.clone(), true).await;
    let (a, user, cookie) = account(&app, "usage-a").await;
    let (_, user_b, cookie_b) = account(&app, "usage-b").await;
    let time = now();
    let hour = time - time % 3_600_000;
    let mut batch = UsageBatch {
        id: Uuid::new_v4(),
        user_id: Some(UserId::from_uuid(
            user["id"].as_str().unwrap().parse().unwrap(),
        )),
        hour_unix_ms: hour,
        counters: UsageCounters {
            uploaded_files: 1,
            uploaded_bytes: 1234,
            ..Default::default()
        },
    };
    let value = serde_json::to_value(&batch).unwrap();
    let (r1, r2) = tokio::join!(
        native_call(&app, "POST", "/api/account/usage", &a, value.clone()),
        native_call(&app, "POST", "/api/account/usage", &a, value)
    );
    assert_eq!(r1.0, StatusCode::NO_CONTENT);
    assert_eq!(r2.0, StatusCode::NO_CONTENT);
    batch.counters.uploaded_bytes = 4321;
    assert_eq!(
        native_call(&app, "POST", "/api/account/usage", &a, json!(batch))
            .await
            .0,
        StatusCode::CONFLICT
    );
    batch.id = Uuid::new_v4();
    batch.counters.relay_upload_bytes = 100;
    assert_eq!(
        native_call(&app, "POST", "/api/account/usage", &a, json!(batch))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    batch.counters = UsageCounters {
        relay_upload_bytes: 100,
        relay_download_bytes: 200,
        ..Default::default()
    };
    control
        .store()
        .record_usage("relay", "test-node", &batch)
        .await
        .unwrap();
    control
        .store()
        .record_usage("relay", "test-node", &batch)
        .await
        .unwrap();
    let path = format!("/api/web/usage?from={hour}&until={}", hour + 3_600_000);
    let result = call(&app, "GET", &path, &cookie, Value::Null).await;
    assert_eq!(result.0, StatusCode::OK, "{}", result.2);
    assert_eq!(result.2["counters"]["uploaded_bytes"], 1234);
    assert_eq!(result.2["counters"]["uploaded_files"], 1);
    assert_eq!(result.2["counters"]["relay_download_bytes"], 200);
    assert_eq!(
        call(&app, "GET", &path, &cookie_b, Value::Null).await.2["counters"]["uploaded_bytes"],
        0
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{path}&scope=all"),
            &cookie,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{path}&user={}", user_b["id"].as_str().unwrap()),
            &cookie,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    batch.id = Uuid::new_v4();
    batch.counters = UsageCounters::default();
    batch.hour_unix_ms = hour + 3_600_000;
    assert_eq!(
        native_call(&app, "POST", "/api/account/usage", &a, json!(batch))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    batch.hour_unix_ms = hour - 31 * 86_400_000;
    assert_eq!(
        native_call(&app, "POST", "/api/account/usage", &a, json!(batch))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    batch.hour_unix_ms = hour;
    batch.user_id = Some(UserId::from_uuid(
        user_b["id"].as_str().unwrap().parse().unwrap(),
    ));
    assert_eq!(
        native_call(&app, "POST", "/api/account/usage", &a, json!(batch))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    pab_server::web::bootstrap_admin(&control, "usage-a")
        .await
        .unwrap();
    let gap = UsageBatch {
        id: Uuid::new_v4(),
        user_id: None,
        hour_unix_ms: hour,
        counters: UsageCounters {
            relay_upload_bytes: 42,
            incomplete: true,
            ..Default::default()
        },
    };
    control
        .store()
        .record_usage("relay", "test-node", &gap)
        .await
        .unwrap();
    let personal = call(&app, "GET", &path, &cookie_b, Value::Null).await.2;
    assert_eq!(personal["counters"]["incomplete"], true);
    assert_eq!(personal["counters"]["relay_upload_bytes"], 0);
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{path}&scope=all&population=guests"),
            &cookie,
            Value::Null
        )
        .await
        .2["counters"]["relay_upload_bytes"],
        42
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{path}&scope=all&population=users"),
            &cookie,
            Value::Null
        )
        .await
        .2["counters"]["relay_upload_bytes"],
        100
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/web/usage?interval=day",
            &cookie,
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{path}&scope=all"),
            &cookie,
            Value::Null
        )
        .await
        .2["counters"]["uploaded_bytes"],
        1234
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/web/usage?interval=lifetime",
            &cookie,
            Value::Null
        )
        .await
        .2["counters"]["uploaded_bytes"],
        1234
    );
    let day = hour - hour % 86_400_000;
    assert_eq!(
        call(
            &app,
            "GET",
            &format!(
                "/api/web/usage?interval=day&from={day}&until={}",
                day + 86_400_000
            ),
            &cookie,
            Value::Null
        )
        .await
        .2["counters"]["uploaded_bytes"],
        1234
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn importing_local_snapshot_never_grants_live_access_or_restores_deletions(pool: PgPool) {
    let (app, _) = app(pool.clone(), true).await;
    let (token, user, cookie) = account(&app, "import-user").await;
    let peer = saved_fixture(&pool, &user).await;
    let input = json!({"device_ref":peer.device_ref,"code":"345678901","name":"local name","system":"windows","alias":"my alias"});
    for _ in 0..2 {
        assert_eq!(
            native_call(
                &app,
                "POST",
                "/api/account/saved-devices/import",
                &token,
                input.clone()
            )
            .await
            .1["revision"],
            1
        );
    }
    let list = call(&app, "GET", "/api/web/saved-devices", &cookie, Value::Null)
        .await
        .2;
    assert!(list["items"][0]["online"].is_null());
    assert_eq!(list["items"][0]["name"], "local name");
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/web/devices/{}", peer.device_ref.device_id),
            &cookie,
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let remove = json!({"id":Uuid::new_v4(),"device_id":peer.device_ref.device_id,"expected_revision":1,"alias":"my alias","deleted":true});
    assert_eq!(
        native_call(&app, "POST", "/api/account/saved-devices", &token, remove)
            .await
            .1["revision"],
        2
    );
    assert_eq!(
        native_call(
            &app,
            "POST",
            "/api/account/saved-devices/import",
            &token,
            input
        )
        .await
        .1["revision"],
        2
    );
    assert_eq!(
        call(&app, "GET", "/api/web/saved-devices", &cookie, Value::Null)
            .await
            .2["items"][0]["deleted"],
        true
    );
}
