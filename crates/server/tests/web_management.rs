use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use pab_protocol::RelayLimitDefaults;
use pab_server::{
    ControlApiConfig, ControlApiState, ControlPlane, PasswordPolicy, PostgresStore,
    RelayControlAuth,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

async fn unclaimed_fixture(pool: &PgPool) -> pab_server::RegisteredEndpoint {
    let tenant = uuid::Uuid::new_v4();
    let device = uuid::Uuid::new_v4();
    let key = pab_protocol::EndpointKey::new([19; 32]);
    sqlx::query("INSERT INTO tenants(id,kind) VALUES($1,'unclaimed_device')")
        .bind(tenant)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO devices(id,tenant_id,name,code) VALUES($1,$2,'Unassigned fixture',345678901)",
    )
    .bind(device)
    .bind(tenant)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO endpoints(endpoint_key,tenant_id,owner_kind,device_id) VALUES($1,$2,'device',$3)").bind(key.as_bytes().as_slice()).bind(tenant).bind(device).execute(pool).await.unwrap();
    pab_server::RegisteredEndpoint {
        endpoint_key: key,
        tenant_id: pab_protocol::TenantId::from_uuid(tenant),
        principal: pab_protocol::EndpointProofPrincipal::Device {
            device_id: pab_protocol::DeviceId::from_uuid(device),
        },
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn removed_claim_routes_cannot_change_devices(pool: PgPool) {
    let (app, control) = app(pool.clone(), true).await;
    unclaimed_fixture(&pool).await;
    let (_, cookie, _) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"former-claim-user","password":"test password long enough"}),
    )
    .await;
    // Both ordinary users and administrators must get 404, including stale requests.
    for admin in [false, true] {
        if admin {
            pab_server::web::bootstrap_admin(&control, "former-claim-user")
                .await
                .unwrap();
        }
        for (method, path) in [
            ("GET", "/api/web/claims".to_owned()),
            ("POST", "/api/web/claims".to_owned()),
            (
                "POST",
                format!("/api/web/claims/{}/cancel", uuid::Uuid::new_v4()),
            ),
        ] {
            assert_eq!(
                call(
                    &app,
                    method,
                    &path,
                    &cookie,
                    json!({"device_code":"345678901","request_id":uuid::Uuid::new_v4()})
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
        }
        let overview = call(&app, "GET", "/api/web/overview", &cookie, Value::Null).await;
        assert_eq!(overview.0, StatusCode::OK);
        assert!(overview.2.get("pendingClaims").is_none());
        assert!(overview.2.get("unclaimed").is_none());
        assert_eq!(
            call(
                &app,
                "GET",
                "/api/web/devices?owner=unclaimed",
                &cookie,
                Value::Null
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let devices = call(
        &app,
        "GET",
        "/api/web/devices?scope=all",
        &cookie,
        Value::Null,
    )
    .await;
    assert_eq!(devices.0, StatusCode::OK);
    assert_eq!(devices.2["total"], 1);
    let device = &devices.2["items"][0];
    assert!(device.get("owner").is_none());
    assert!(device.get("owner_id").is_none());
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("/api/web/devices/{}", device["id"].as_str().unwrap()),
            &cookie,
            json!({"name":"Managed without claiming","revision":device["revision"]})
        )
        .await
        .0,
        StatusCode::OK
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM device_claim_requests")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn legacy_control_claim_requests_are_rejected_without_disconnect(pool: PgPool) {
    use pab_protocol::{
        ClaimId, ControlClientMessage as Client, ControlServerMessage as Server, RequestId,
        TenantId,
    };
    let (_, control) = app(pool.clone(), true).await;
    unclaimed_fixture(&pool).await;
    let mut session = pab_server::ControlSession::new(control, ControlApiConfig::default());
    let request_id = RequestId::new();
    for request in [
        Client::BeginDeviceClaim {
            request_id,
            device_code: "345678901".parse().unwrap(),
            owner_tenant_id: TenantId::new(),
        },
        Client::ListDeviceClaims { request_id },
        Client::ApproveDeviceClaim {
            request_id,
            claim_id: ClaimId::new(),
        },
        Client::RejectDeviceClaim {
            request_id,
            claim_id: ClaimId::new(),
        },
    ] {
        assert!(
            matches!(session.handle(request).await, Server::Error { request_id:Some(id), code:pab_protocol::ControlErrorCode::InvalidMessage, .. } if id == request_id)
        );
    }
    assert!(matches!(
        session
            .handle(Client::RegisterAccount {
                request_id,
                username: "after-removed-claim".into(),
                password: "test password long enough".into()
            })
            .await,
        Server::AccountAuthenticated { .. }
    ));
    let owner: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT owner_tenant_id FROM devices WHERE code=345678901")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(owner.is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn removed_unbind_preserves_existing_access(pool: PgPool) {
    let (app, control) = app(pool.clone(), true).await;
    let endpoint = unclaimed_fixture(&pool).await;
    let pab_protocol::EndpointProofPrincipal::Device { device_id } = endpoint.principal else {
        panic!("expected device")
    };
    let (_, cookie, me) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"existing-owner","password":"test password long enough"}),
    )
    .await;
    let user = uuid::Uuid::parse_str(me["id"].as_str().unwrap()).unwrap();
    // Seed ownership that predates removal of the claim feature.
    sqlx::query("UPDATE devices SET owner_tenant_id=(SELECT tenant_id FROM personal_tenants WHERE user_id=$1) WHERE id=$2")
        .bind(user).bind(device_id.as_uuid()).execute(&pool).await.unwrap();
    let path = format!("/api/web/devices/{device_id}");
    let detail = call(&app, "GET", &path, &cookie, Value::Null).await;
    assert_eq!(detail.0, StatusCode::OK);
    let (_, stranger, _) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"other-owner","password":"test password long enough"}),
    )
    .await;
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path}/unbind"),
            &stranger,
            json!({"revision":detail.2["revision"]})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path}/unbind"),
            &cookie,
            json!({"revision":detail.2["revision"]})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "GET", &path, &cookie, Value::Null).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", &path, &stranger, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
    assert!(detail.2.get("owner").is_none());
    assert!(detail.2.get("owner_id").is_none());
    pab_server::web::bootstrap_admin(&control, "existing-owner")
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path}/unbind"),
            &cookie,
            json!({"revision":detail.2["revision"]})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let identity: (uuid::Uuid, i32, Option<uuid::Uuid>) =
        sqlx::query_as("SELECT tenant_id,code,owner_tenant_id FROM devices WHERE id=$1")
            .bind(device_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        identity,
        (
            endpoint.tenant_id.as_uuid(),
            345678901,
            Some(uuid::Uuid::parse_str(me["personal_tenant_id"].as_str().unwrap()).unwrap())
        )
    );
}

async fn app(pool: PgPool, registration_enabled: bool) -> (Router, ControlPlane) {
    let control =
        ControlPlane::new(PostgresStore::from_pool(pool), PasswordPolicy::default()).unwrap();
    control
        .initialize_settings(RelayLimitDefaults {
            team_mbps: 20,
            member_mbps: 4,
            personal_mbps: 5,
        })
        .await
        .unwrap();
    let state = ControlApiState::new(
        control.clone(),
        ControlApiConfig {
            registration_enabled,
            ..Default::default()
        },
        RelayControlAuth::new("web-test-secret-with-at-least-32-characters").unwrap(),
    );
    (pab_server::control_router(state), control)
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    body: Value,
) -> (StatusCode, String, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "web.example")
        .header(header::ORIGIN, "https://web.example")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookie)
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({"raw":String::from_utf8_lossy(&bytes)}));
    (status, cookie, body)
}

#[sqlx::test(migrations = "./migrations")]
async fn browser_session_register_restore_logout_and_origin(pool: PgPool) {
    let (app, _) = app(pool.clone(), true).await;
    let (status, cookie, user) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"web-user","password":"correct horse battery staple"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{user}");
    assert_eq!(user["server_admin"], false);
    assert!(cookie.contains("HttpOnly; Secure; SameSite=Strict"));
    assert!(cookie.starts_with("__Host-pab_session="));
    let token = cookie.split(';').next().unwrap();
    let restored = call(&app, "GET", "/api/web/session", token, Value::Null).await;
    assert_eq!(restored.0, StatusCode::OK);
    assert_eq!(restored.2["id"], user["id"]);
    let stored: String = sqlx::query_scalar("SELECT token_hash FROM web_sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!token.contains(&stored));
    let attack = Request::builder()
        .method("POST")
        .uri("/api/web/logout")
        .header(header::HOST, "web.example")
        .header(header::ORIGIN, "https://attacker.example")
        .header(header::COOKIE, token)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(attack).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "POST", "/api/web/logout", token, Value::Null)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&app, "GET", "/api/web/session", token, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn password_change_revokes_all_sessions_and_admin_is_explicit(pool: PgPool) {
    let (app, control) = app(pool, true).await;
    let creds = json!({"username":"web-admin","password":"original password long enough"});
    let (_, first, _) = call(&app, "POST", "/api/web/register", "", creds.clone()).await;
    let (_, second, _) = call(&app, "POST", "/api/web/session", "", creds).await;
    pab_server::web::bootstrap_admin(&control, "web-admin")
        .await
        .unwrap();
    let current = call(&app, "GET", "/api/web/session", &first, Value::Null).await;
    assert_eq!(current.2["server_admin"], true);
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/web/password",
            &first,
            json!({"current_password":"wrong","new_password":"updated password long enough"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let changed=call(&app,"POST","/api/web/password",&first,json!({"current_password":"original password long enough","new_password":"updated password long enough"})).await;
    assert_eq!(changed.0, StatusCode::NO_CONTENT, "{}", changed.2);
    for cookie in [first, second] {
        assert_eq!(
            call(&app, "GET", "/api/web/session", &cookie, Value::Null)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/web/session",
            "",
            json!({"username":"web-admin","password":"updated password long enough"})
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn registration_disabled_expired_and_disabled_accounts(pool: PgPool) {
    let (app, control) = app(pool.clone(), false).await;
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/web/register",
            "",
            json!({"username":"closed","password":"correct horse battery staple"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    control
        .register_account("existing", "correct horse battery staple")
        .await
        .unwrap();
    let creds = json!({"username":"existing","password":"correct horse battery staple"});
    let (_, cookie, _) = call(&app, "POST", "/api/web/session", "", creds.clone()).await;
    sqlx::query("UPDATE web_sessions SET created_at=now()-interval '2 days',expires_at=now()-interval '1 day'").execute(&pool).await.unwrap();
    assert_eq!(
        call(&app, "GET", "/api/web/session", &cookie, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (_, cookie, _) = call(&app, "POST", "/api/web/session", "", creds.clone()).await;
    sqlx::query("UPDATE users SET status='disabled'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(&app, "GET", "/api/web/session", &cookie, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "POST", "/api/web/session", "", creds).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn device_lists_use_current_ownership_and_exact_filtered_totals(pool: PgPool) {
    let (app, control) = app(pool.clone(), true).await;
    let (_, cookie, me) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"device-owner","password":"test password long enough"}),
    )
    .await;
    let other = control
        .register_account("other-owner", "test password long enough")
        .await
        .unwrap();
    let tenant = uuid::Uuid::parse_str(me["personal_tenant_id"].as_str().unwrap()).unwrap();
    let user = uuid::Uuid::parse_str(me["id"].as_str().unwrap()).unwrap();
    let mut first = uuid::Uuid::nil();
    for index in 0..21 {
        let id = uuid::Uuid::new_v4();
        if index == 0 {
            first = id;
        }
        sqlx::query("INSERT INTO devices(id,tenant_id,owner_tenant_id,registered_by_user_id,code,name) VALUES($1,$2,$2,$3,$4,$5)")
            .bind(id).bind(tenant).bind(user).bind(123456700+index).bind(format!("Fixture {index:02}")).execute(&pool).await.unwrap();
    }
    let listed = call(&app, "GET", "/api/web/devices", &cookie, Value::Null).await;
    assert_eq!(listed.0, StatusCode::OK, "{}", listed.2);
    assert_eq!(listed.2["total"], 21);
    assert_eq!(listed.2["items"].as_array().unwrap().len(), 20);
    assert_eq!(
        call(&app, "GET", "/api/web/devices?page=2", &cookie, Value::Null)
            .await
            .2["total"],
        21
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/web/devices?q=123%20456%20700",
            &cookie,
            Value::Null
        )
        .await
        .2["total"],
        1
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/web/devices?status=online",
            &cookie,
            Value::Null
        )
        .await
        .2["total"],
        0
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/web/devices?page_size=0",
            &cookie,
            Value::Null
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/web/devices?scope=all",
            &cookie,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    sqlx::query("UPDATE devices SET owner_tenant_id=$1 WHERE id=$2")
        .bind(other.personal_tenant_id.as_uuid())
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/web/devices/{first}"),
            &cookie,
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "GET", "/api/web/devices", &cookie, Value::Null)
            .await
            .2["total"],
        20
    );
    pab_server::web::bootstrap_admin(&control, "device-owner")
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/web/devices?scope=all",
            &cookie,
            Value::Null
        )
        .await
        .2["total"],
        21
    );
    let updated = call(
        &app,
        "PATCH",
        &format!("/api/web/devices/{first}"),
        &cookie,
        json!({"name":"新名称","revision":1}),
    )
    .await;
    assert_eq!(updated.0, StatusCode::OK, "{}", updated.2);
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("/api/web/devices/{first}"),
            &cookie,
            json!({"name":"stale","revision":1})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn team_web_management_obeys_membership_and_audits_real_changes(pool: PgPool) {
    let (app, control) = app(pool, true).await;
    let (_, admin_cookie, admin) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"team-web-admin","password":"test password long enough"}),
    )
    .await;
    let (_, member_cookie, member) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"team-web-member","password":"test password long enough"}),
    )
    .await;
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/web/accounts",
            &member_cookie,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    pab_server::web::bootstrap_admin(&control, "team-web-admin")
        .await
        .unwrap();
    let create = call(
        &app,
        "POST",
        "/api/web/teams",
        &admin_cookie,
        json!({"name":"Web Team","owner_id":admin["id"]}),
    )
    .await;
    assert_eq!(create.0, StatusCode::OK, "{}", create.2);
    let team = create.2["id"].as_str().unwrap();
    assert_eq!(
        call(&app, "GET", "/api/web/teams", &member_cookie, Value::Null)
            .await
            .2["total"],
        0
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/web/teams/{team}/actions"),
            &admin_cookie,
            json!({"action":"add_member","user_id":member["id"],"role":"member"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", "/api/web/teams", &member_cookie, Value::Null)
            .await
            .2["total"],
        1
    );
    let members = call(
        &app,
        "GET",
        &format!("/api/web/teams/{team}/members"),
        &admin_cookie,
        Value::Null,
    )
    .await;
    assert_eq!(members.0, StatusCode::OK, "{}", members.2);
    assert_eq!(members.2["total"], 2);
    let assignment = format!(
        "/api/web/accounts/{}/traffic",
        member["id"].as_str().unwrap()
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &assignment,
            &admin_cookie,
            json!({"team_id":team})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/web/teams/{team}/actions"),
            &admin_cookie,
            json!({"action":"set_limits","total_mbps":30,"member_mbps":6})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", "/api/web/traffic", &member_cookie, Value::Null)
            .await
            .2["scopes"]["default_tenant_id"],
        team
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/web/teams/{team}/actions"),
            &admin_cookie,
            json!({"action":"remove_member","user_id":member["id"]})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", "/api/web/traffic", &member_cookie, Value::Null)
            .await
            .2["scopes"]["default_tenant_id"],
        member["personal_tenant_id"]
    );
    let events = call(&app, "GET", "/api/web/audit", &admin_cookie, Value::Null).await;
    assert_eq!(events.0, StatusCode::OK, "{}", events.2);
    assert!(events.2["total"].as_i64().unwrap() >= 6);
    assert_eq!(
        call(&app, "GET", "/api/web/audit", &member_cookie, Value::Null)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_admin_changes_keep_an_active_administrator(pool: PgPool) {
    let (app, control) = app(pool.clone(), true).await;
    let (_, a, ua) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"admin-a","password":"test password long enough"}),
    )
    .await;
    let (_, b, ub) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"admin-b","password":"test password long enough"}),
    )
    .await;
    pab_server::web::bootstrap_admin(&control, "admin-a")
        .await
        .unwrap();
    pab_server::web::bootstrap_admin(&control, "admin-b")
        .await
        .unwrap();
    let patha = format!("/api/web/accounts/{}", ua["id"].as_str().unwrap());
    let pathb = format!("/api/web/accounts/{}", ub["id"].as_str().unwrap());
    let change = json!({"status":"active","server_admin":false,"revision":1});
    let (a, b) = tokio::join!(
        call(&app, "PATCH", &patha, &a, change.clone()),
        call(&app, "PATCH", &pathb, &b, change)
    );
    assert!([a.0, b.0].contains(&StatusCode::OK));
    assert!([a.0, b.0].contains(&StatusCode::CONFLICT));
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE server_admin AND status='active'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn input_boundaries_privileged_configuration_and_no_task_routes(pool: PgPool) {
    let (app, control) = app(pool, true).await;
    let (_, cookie, _) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"boundary-user","password":"test password long enough"}),
    )
    .await;
    for path in [
        "/api/web/service",
        "/api/web/relays",
        "/api/web/accounts",
        "/api/web/audit",
        "/api/web/devices?scope=all",
    ] {
        assert_eq!(
            call(&app, "GET", path, &cookie, Value::Null).await.0,
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    for path in [
        "/api/web/devices?page=0",
        "/api/web/devices?page_size=101",
        "/api/web/devices?status=anything",
        "/api/web/devices?sort=name;DROP",
        "/api/web/devices/not-a-uuid",
    ] {
        let response = call(&app, "GET", path, &cookie, Value::Null).await;
        assert!(response.0.is_client_error(), "{path}");
        assert_eq!(response.2["code"], "invalid_input");
    }
    for path in [
        "/api",
        "/api/web/tasks",
        "/api/web/task-events",
        "/api/tasks",
    ] {
        assert_eq!(
            call(&app, "GET", path, &cookie, Value::Null).await.0,
            StatusCode::NOT_FOUND
        );
    }
    let large = Request::builder()
        .method("POST")
        .uri("/api/web/register")
        .header("host", "web.example")
        .header("origin", "https://web.example")
        .header("content-type", "application/json")
        .body(Body::from("x".repeat(17000)))
        .unwrap();
    let response = app.clone().oneshot(large).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(response.headers()["cache-control"], "no-store");
    pab_server::web::bootstrap_admin(&control, "boundary-user")
        .await
        .unwrap();
    let (_, _, config) = call(&app, "GET", "/api/web/service", &cookie, Value::Null).await;
    assert_eq!(config["default_team_mbps"], 20);
    assert_eq!(config["session_hours"], 12);
    for forbidden in ["secret", "password", "database", "token"] {
        assert!(!config.to_string().contains(forbidden));
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn relay_expiry_restart_and_thousand_device_pagination(pool: PgPool) {
    let (app, control) = app(pool.clone(), true).await;
    let (_, cookie, user) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"scale-admin","password":"test password long enough"}),
    )
    .await;
    pab_server::web::bootstrap_admin(&control, "scale-admin")
        .await
        .unwrap();
    let tenant = uuid::Uuid::parse_str(user["personal_tenant_id"].as_str().unwrap()).unwrap();
    sqlx::query("INSERT INTO devices(id,tenant_id,owner_tenant_id,code,name) SELECT gen_random_uuid(),$1,$1,700000000+i,'Load device '||lpad(i::text,4,'0') FROM generate_series(1,1000) i").bind(tenant).execute(&pool).await.unwrap();
    let start = std::time::Instant::now();
    let mut ids = std::collections::HashSet::new();
    for page in 1..=50 {
        let response = call(
            &app,
            "GET",
            &format!("/api/web/devices?page={page}"),
            &cookie,
            Value::Null,
        )
        .await;
        assert_eq!(response.0, StatusCode::OK);
        assert_eq!(response.2["total"], 1000);
        for item in response.2["items"].as_array().unwrap() {
            assert!(ids.insert(item["id"].as_str().unwrap().to_owned()));
        }
    }
    assert_eq!(ids.len(), 1000);
    eprintln!(
        "1000-device/50-page sequential query time: {:?}",
        start.elapsed()
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/web/devices?page=51",
            &cookie,
            Value::Null
        )
        .await
        .2["items"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    // A heartbeat from an earlier Server instance must never appear online after restart.
    sqlx::query("INSERT INTO relay_nodes(node_id,offered_policy_version,last_seen_at,server_instance) VALUES('old-instance',1,now(),$1),('expired',1,now()-interval '3 minutes',$1)").bind(uuid::Uuid::new_v4()).execute(&pool).await.unwrap();
    let response = call(
        &app,
        "GET",
        "/api/web/relays?page_size=1",
        &cookie,
        Value::Null,
    )
    .await;
    assert_eq!(response.2["total"], 2);
    assert_eq!(response.2["items"][0]["online"], false);
    assert_eq!(
        call(&app, "GET", "/api/web/overview", &cookie, Value::Null)
            .await
            .2["relays"],
        0
    );
}

#[sqlx::test(migrations = false)]
async fn upgrade_from_twelve_preserves_accounts_devices_and_teams(pool: PgPool) {
    let mut old = sqlx::migrate!("./migrations");
    old.migrations =
        std::borrow::Cow::Owned(old.iter().filter(|m| m.version <= 12).cloned().collect());
    old.run(&pool).await.unwrap();
    let (_, control) = app(pool.clone(), true).await;
    let account = control
        .register_account("upgrade-user", "test password long enough")
        .await
        .unwrap();
    let team_id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, kind, created_by_user_id) VALUES ($1, 'team', $2)")
        .bind(team_id)
        .bind(account.id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO teams (tenant_id, name) VALUES ($1, 'Preserved Team')")
        .bind(team_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO memberships (tenant_id, user_id, role) VALUES ($1, $2, 'owner')")
        .bind(team_id)
        .bind(account.id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let endpoint = unclaimed_fixture(&pool).await;
    let pab_protocol::EndpointProofPrincipal::Device { device_id } = endpoint.principal else {
        panic!("expected device")
    };
    sqlx::query("INSERT INTO device_claim_requests(id,device_id,owner_tenant_id,requested_by_user_id,expires_at) VALUES($1,$2,$3,$4,now()+interval '10 minutes')")
        .bind(uuid::Uuid::new_v4()).bind(device_id.as_uuid()).bind(account.personal_tenant_id.as_uuid()).bind(account.id.as_uuid()).execute(&pool).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap(); // Restart is idempotent.
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM teams WHERE tenant_id=$1 AND name='Preserved Team'",
    )
    .bind(team_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let resolution: String =
        sqlx::query_scalar("SELECT resolution FROM device_claim_requests WHERE device_id=$1")
            .bind(device_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(resolution, "cancelled");
    let identity: (uuid::Uuid, i32, Option<uuid::Uuid>) =
        sqlx::query_as("SELECT tenant_id,code,owner_tenant_id FROM devices WHERE id=$1")
            .bind(device_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(identity, (endpoint.tenant_id.as_uuid(), 345678901, None));
    let (app, _) = app_from_existing(control);
    let response = call(
        &app,
        "POST",
        "/api/web/session",
        "",
        json!({"username":"upgrade-user","password":"test password long enough"}),
    )
    .await;
    assert_eq!(response.0, StatusCode::OK);
    assert_eq!(response.2["server_admin"], false);
}

fn app_from_existing(control: ControlPlane) -> (Router, ControlPlane) {
    let state = ControlApiState::new(
        control.clone(),
        ControlApiConfig::default(),
        RelayControlAuth::new("web-test-secret-with-at-least-32-characters").unwrap(),
    );
    (pab_server::control_router(state), control)
}

#[sqlx::test(migrations = "./migrations")]
async fn event_subscriptions_are_bounded_and_release_capacity(pool: PgPool) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{Message, client::IntoClientRequest},
    };
    let (app, _) = app(pool, true).await;
    let (_, cookie, _) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"ws-budget","password":"test password long enough"}),
    )
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let request = || {
        let mut req = format!("ws://{address}/api/web/events")
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert("origin", format!("https://{address}").parse().unwrap());
        req.headers_mut()
            .insert("cookie", cookie.split(';').next().unwrap().parse().unwrap());
        req
    };
    let mut sockets = Vec::new();
    for _ in 0..256 {
        let (mut socket, _) = connect_async(request()).await.unwrap();
        let initial = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Text(text) = initial else {
            panic!("missing refresh")
        };
        let body: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body.as_object().unwrap().len(), 2);
        assert_eq!(body["type"], "refresh");
        sockets.push(socket);
    }
    let rejected = connect_async(request()).await.unwrap_err();
    assert!(
        matches!(rejected,tokio_tungstenite::tungstenite::Error::Http(response) if response.status()==StatusCode::TOO_MANY_REQUESTS)
    );
    let mut released = sockets.pop().unwrap();
    released.send(Message::Close(None)).await.unwrap();
    drop(released);
    let mut replacement = None;
    for _ in 0..20 {
        if let Ok((socket, _)) = connect_async(request()).await {
            replacement = Some(socket);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(replacement.is_some());
    drop(replacement);
    drop(sockets);
    server.abort();
}
