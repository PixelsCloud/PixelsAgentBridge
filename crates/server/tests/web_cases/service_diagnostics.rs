use super::*;
use uuid::Uuid;

struct Fixture {
    app: Router,
    control: ControlPlane,
    admin: String,
    user: String,
    user_id: Uuid,
}
async fn fixture(pool: &PgPool) -> Fixture {
    let (app, control) = app(pool.clone(), true).await;
    let (_, admin, _) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"policy-admin","password":"isolated policy test password"}),
    )
    .await;
    let (_, user, value) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"policy-user","password":"isolated policy test password"}),
    )
    .await;
    pab_server::web::bootstrap_admin(&control, "policy-admin")
        .await
        .unwrap();
    Fixture {
        app,
        control,
        admin,
        user,
        user_id: value["id"].as_str().unwrap().parse().unwrap(),
    }
}
async fn policy(f: &Fixture, revision: i64, value: Value) -> (StatusCode, String, Value) {
    call(
        &f.app,
        "PATCH",
        "/api/web/service",
        &f.admin,
        json!({"revision":revision,"registration_override":value}),
    )
    .await
}

#[sqlx::test(migrations = "./migrations")]
async fn runtime_registration_policy_covers_public_configs_http_and_legacy_control(pool: PgPool) {
    use pab_protocol::{
        ControlClientMessage as Client, ControlErrorCode, ControlServerMessage as Server, RequestId,
    };
    let f = fixture(&pool).await;
    let initial = call(&f.app, "GET", "/api/web/service", &f.admin, Value::Null)
        .await
        .2;
    assert_eq!(initial["registration"]["deployment_default"], true);
    assert!(initial["registration"]["override_value"].is_null());
    let closed = policy(&f, 1, json!(false)).await;
    assert_eq!(closed.0, StatusCode::OK);
    assert_eq!(closed.2["registration"]["source"], "runtime_override");
    for path in ["/api/web/config", "/api/account/config"] {
        assert_eq!(
            call(&f.app, "GET", path, "", Value::Null).await.2["registration_enabled"],
            false
        );
    }
    for path in ["/api/web/register", "/api/account/register"] {
        let result = call(
            &f.app,
            "POST",
            path,
            "",
            json!({"username":"must-not-exist","password":"isolated policy test password"}),
        )
        .await;
        assert_eq!(result.0, StatusCode::FORBIDDEN);
        assert_eq!(result.2["code"], "registration_disabled");
    }
    let mut control =
        pab_server::ControlSession::new(f.control.clone(), ControlApiConfig::default());
    assert!(matches!(
        control
            .handle(Client::RegisterAccount {
                request_id: RequestId::new(),
                username: "legacy-denied".into(),
                password: "isolated policy test password".into()
            })
            .await,
        Server::Error {
            code: ControlErrorCode::RegistrationDisabled,
            ..
        }
    ));
    assert_eq!(
        call(
            &f.app,
            "POST",
            "/api/web/session",
            "",
            json!({"username":"policy-user","password":"isolated policy test password"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&f.app, "GET", "/api/web/session", &f.user, Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        native_call(
            &f.app,
            "POST",
            "/api/account/session",
            "",
            json!({"username":"policy-user","password":"isolated policy test password"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let restored = policy(&f, 2, Value::Null).await;
    assert_eq!(restored.0, StatusCode::OK);
    assert_eq!(restored.2["registration"]["effective"], true);
    assert_eq!(restored.2["registration"]["source"], "deployment_default");
    assert!(matches!(
        control
            .handle(Client::RegisterAccount {
                request_id: RequestId::new(),
                username: "legacy-reopened".into(),
                password: "isolated policy test password".into()
            })
            .await,
        Server::AccountAuthenticated { .. }
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn policy_override_survives_router_restart_and_reset_uses_new_deployment_default(
    pool: PgPool,
) {
    let f = fixture(&pool).await;
    assert_eq!(policy(&f, 1, json!(true)).await.0, StatusCode::OK);
    let (restarted, _) = app(pool.clone(), false).await;
    let value = call(&restarted, "GET", "/api/web/service", &f.admin, Value::Null)
        .await
        .2;
    assert_eq!(value["registration"]["deployment_default"], false);
    assert_eq!(value["registration_enabled"], true);
    assert_eq!(value["registration"]["revision"], 2);
    assert_eq!(
        call(
            &restarted,
            "POST",
            "/api/account/register",
            "",
            json!({"username":"override-open","password":"isolated policy test password"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let reset = call(
        &restarted,
        "PATCH",
        "/api/web/service",
        &f.admin,
        json!({"revision":2,"registration_override":null}),
    )
    .await;
    assert_eq!(reset.0, StatusCode::OK);
    assert_eq!(reset.2["registration_enabled"], false);
    let (again, _) = app(pool.clone(), false).await;
    assert_eq!(
        call(&again, "GET", "/api/account/config", "", Value::Null)
            .await
            .2["registration_enabled"],
        false
    );
    let audit = call(
        &again,
        "GET",
        "/api/web/audit?action=service.registration_updated&result=succeeded",
        &f.admin,
        Value::Null,
    )
    .await
    .2;
    assert_eq!(audit["total"], 2);
    assert_eq!(
        audit["items"][0]["before"]["registration_source"],
        "runtime_override"
    );
    assert_eq!(
        audit["items"][0]["after"]["registration_source"],
        "deployment_default"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn simultaneous_policy_edits_conflict_and_audit_failure_rolls_back(pool: PgPool) {
    let f = fixture(&pool).await;
    let (a, b) = tokio::join!(policy(&f, 1, json!(false)), policy(&f, 1, json!(true)));
    let statuses = [a.0, b.0];
    assert!(statuses.contains(&StatusCode::OK));
    assert!(statuses.contains(&StatusCode::CONFLICT));
    let before = call(&f.app, "GET", "/api/web/service", &f.admin, Value::Null)
        .await
        .2;
    sqlx::raw_sql("CREATE FUNCTION reject_policy_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.action='service.registration_updated' AND NEW.result='succeeded' THEN RAISE EXCEPTION 'isolated policy audit fault'; END IF; RETURN NEW; END $$; CREATE TRIGGER policy_audit_fault BEFORE INSERT ON web_admin_events FOR EACH ROW EXECUTE FUNCTION reject_policy_audit();").execute(&pool).await.unwrap();
    assert_eq!(
        policy(&f, 2, Value::Null).await.0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let after = call(&f.app, "GET", "/api/web/service", &f.admin, Value::Null)
        .await
        .2;
    assert_eq!(before, after);
    let audit = call(
        &f.app,
        "GET",
        "/api/web/audit?action=service.registration_updated&result=succeeded",
        &f.admin,
        Value::Null,
    )
    .await
    .2;
    assert_eq!(audit["total"], 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn policy_rejects_missing_override_nonadmin_and_revoked_waiting_actor(pool: PgPool) {
    let f = fixture(&pool).await;
    for body in [
        json!({"revision":1}),
        json!({"revision":1,"registration_override":"false"}),
        json!({"revision":1,"registration_override":false,"secret":"do not store"}),
    ] {
        let status = call(&f.app, "PATCH", "/api/web/service", &f.admin, body)
            .await
            .0;
        assert!(status.is_client_error());
    }
    for (method, path, body) in [
        ("GET", "/api/web/service", Value::Null),
        (
            "PATCH",
            "/api/web/service",
            json!({"revision":1,"registration_override":false}),
        ),
    ] {
        assert_eq!(
            call(&f.app, method, path, &f.user, body.clone()).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(&f.app, method, path, "", body).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let mut gate = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(26035001)")
        .execute(&mut *gate)
        .await
        .unwrap();
    let app = f.app.clone();
    let cookie = f.admin.clone();
    let waiting = tokio::spawn(async move {
        call(
            &app,
            "PATCH",
            "/api/web/service",
            &cookie,
            json!({"revision":1,"registration_override":false}),
        )
        .await
    });
    wait_blocked(&pool, "SELECT pg_advisory_xact_lock(26035001)").await;
    let token = f
        .admin
        .split(';')
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1;
    sqlx::query("DELETE FROM web_sessions WHERE token_hash=$1")
        .bind(blake3::hash(token.as_bytes()).to_hex().to_string())
        .execute(&mut *gate)
        .await
        .unwrap();
    gate.commit().await.unwrap();
    assert_eq!(waiting.await.unwrap().0, StatusCode::FORBIDDEN);
    assert!(
        f.control
            .store()
            .registration_policy(true)
            .await
            .unwrap()
            .override_value
            .is_none()
    );
}
async fn wait_blocked(pool: &PgPool, statement: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(10),async{
        loop {let n:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND query=$1 AND wait_event_type='Lock'").bind(statement).fetch_one(pool).await.unwrap();if n>0{return;}tokio::time::sleep(std::time::Duration::from_millis(20)).await;}
    }).await.expect("expected database lock wait");
}

#[sqlx::test(migrations = "./migrations")]
async fn registration_rechecks_policy_after_password_hashing_at_commit_boundary(pool: PgPool) {
    let f = fixture(&pool).await;
    let mut gate = pool.begin().await.unwrap();
    sqlx::query("SELECT singleton FROM server_settings WHERE singleton FOR UPDATE")
        .execute(&mut *gate)
        .await
        .unwrap();
    let app = f.app.clone();
    let pending = tokio::spawn(async move {
        call(
            &app,
            "POST",
            "/api/web/register",
            "",
            json!({"username":"racing-registration","password":"isolated policy test password"}),
        )
        .await
    });
    wait_blocked(
        &pool,
        "SELECT registration_override FROM server_settings WHERE singleton FOR SHARE",
    )
    .await;
    sqlx::query("UPDATE server_settings SET registration_override=false,registration_revision=2 WHERE singleton").execute(&mut *gate).await.unwrap();
    gate.commit().await.unwrap();
    let result = pending.await.unwrap();
    assert_eq!(result.0, StatusCode::FORBIDDEN);
    assert_eq!(result.2["code"], "registration_disabled");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE username='racing-registration'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

async fn seed_device(pool: &PgPool, index: i32, context: Option<Value>, version: &str) -> Uuid {
    let tenant = Uuid::new_v4();
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants(id,kind) VALUES($1,'unclaimed_device')")
        .bind(tenant)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO devices(id,tenant_id,name,code) VALUES($1,$2,$3,$4)")
        .bind(id)
        .bind(tenant)
        .bind(format!("diagnostic device {index}"))
        .bind(456000000 + index)
        .execute(pool)
        .await
        .unwrap();
    if let Some(context) = context {
        sqlx::query("INSERT INTO device_runtime(tenant_id,device_id,execution_context,environment_revision,agent_version,observed_at_unix_ms) VALUES($1,$2,$3,'fixture', $4,1)").bind(tenant).bind(id).bind(context).bind(version).execute(pool).await.unwrap();
    }
    id
}

#[sqlx::test(migrations = "./migrations")]
async fn device_diagnostics_are_scoped_filtered_and_do_not_infer_permissions(pool: PgPool) {
    let f = fixture(&pool).await;
    let android=seed_device(&pool,1,Some(json!({"os_family":"android","architecture":"aarch64","cwd":"private task path","interpreter":{"executable_path":"private executable"}})),"1.2.3").await;
    seed_device(
        &pool,
        2,
        Some(json!({"os_family":"windows","architecture":"x86_64"})),
        "1.2.4",
    )
    .await;
    seed_device(&pool, 3, None, "").await;
    sqlx::query("INSERT INTO device_accounts(device_id,user_id,revision) VALUES($1,$2,1)")
        .bind(android)
        .bind(f.user_id)
        .execute(&pool)
        .await
        .unwrap();
    for query in [
        "scope=all",
        "scope=all&system=android",
        "scope=all&architecture=x86_64",
        "scope=all&version=1.2.3",
        "scope=all&version_state=unknown",
        "scope=all&system=unknown",
        "scope=all&user=00000000-0000-0000-0000-000000000000",
    ] {
        let list = call(
            &f.app,
            "GET",
            &format!("/api/web/devices?{query}"),
            &f.admin,
            Value::Null,
        )
        .await;
        let stats = call(
            &f.app,
            "GET",
            &format!("/api/web/devices/diagnostics?{query}"),
            &f.admin,
            Value::Null,
        )
        .await;
        assert_eq!(list.0, StatusCode::OK);
        assert_eq!(stats.0, StatusCode::OK, "{}", stats.2);
        assert_eq!(stats.2["total"], list.2["total"]);
        assert_eq!(stats.2["permissions_unknown"], stats.2["total"]);
        assert!(!stats.2.to_string().contains("private"));
    }
    let personal = call(
        &f.app,
        "GET",
        "/api/web/devices/diagnostics?scope=mine",
        &f.user,
        Value::Null,
    )
    .await
    .2;
    assert_eq!(personal["total"], 1);
    assert_eq!(personal["reported"], 1);
    assert_eq!(personal["systems"], json!([{"value":"android","count":1}]));
    assert_eq!(
        call(
            &f.app,
            "GET",
            "/api/web/devices/diagnostics?scope=all",
            &f.user,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let device = call(
        &f.app,
        "GET",
        &format!("/api/web/devices/{android}"),
        &f.admin,
        Value::Null,
    )
    .await
    .2;
    assert_eq!(device["associated_user_id"], f.user_id.to_string());
    assert_eq!(device["associated_username"], "policy-user");
    assert_eq!(device["diagnostics"]["permissions"], "not_reported");
    assert_eq!(device["diagnostics"]["compatibility"], "not_reported");
    assert!(device["diagnostics"]["reported_at"].is_number());
    assert!(!device.to_string().contains("private"));
}

#[sqlx::test(migrations = "./migrations")]
async fn device_version_distribution_is_bounded_and_old_clients_remain_unknown(pool: PgPool) {
    let f = fixture(&pool).await;
    for index in 1..=102 {
        seed_device(&pool, index, Some(json!({})), &format!("fixture-{index}")).await;
    }
    let old = seed_device(&pool, 103, None, "").await;
    let stats = call(
        &f.app,
        "GET",
        "/api/web/devices/diagnostics?scope=all",
        &f.admin,
        Value::Null,
    )
    .await
    .2;
    assert_eq!(stats["total"], 103);
    assert_eq!(stats["not_reported"], 1);
    assert_eq!(stats["versions"].as_array().unwrap().len(), 100);
    assert_eq!(stats["versions_truncated"], true);
    assert_eq!(stats["systems"], json!([{"value":null,"count":103}]));
    let device = call(
        &f.app,
        "GET",
        &format!("/api/web/devices/{old}"),
        &f.admin,
        Value::Null,
    )
    .await
    .2;
    assert_eq!(device["diagnostics"]["runtime_reported"], false);
    assert!(device["diagnostics"]["reported_at"].is_null());
    for query in [
        "version_state=bad",
        "architecture=secret",
        "system=bad",
        "version=1&version_state=unknown",
    ] {
        assert_eq!(
            call(
                &f.app,
                "GET",
                &format!("/api/web/devices/diagnostics?{query}"),
                &f.admin,
                Value::Null
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
}
