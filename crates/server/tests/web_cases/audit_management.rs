use super::*;
use sqlx::Row;
use uuid::Uuid;

struct Fixture {
    app: Router,
    admin: String,
    user: String,
    actor: Uuid,
    target: Uuid,
}
async fn fixture(pool: &PgPool) -> Fixture {
    let (app, control) = app(pool.clone(), true).await;
    let (_, admin, account) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"audit-admin","password":"isolated audit fixture password"}),
    )
    .await;
    let (_, user, target) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"audit-target","password":"isolated audit fixture password"}),
    )
    .await;
    pab_server::web::bootstrap_admin(&control, "audit-admin")
        .await
        .unwrap();
    Fixture {
        app,
        admin,
        user,
        actor: account["id"].as_str().unwrap().parse().unwrap(),
        target: target["id"].as_str().unwrap().parse().unwrap(),
    }
}
async fn list(f: &Fixture, query: &str) -> Value {
    let (status, _, value) = call(
        &f.app,
        "GET",
        &format!("/api/web/audit?{query}"),
        &f.admin,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    value
}
async fn csv(f: &Fixture, query: &str) -> (axum::http::HeaderMap, String) {
    let request = Request::builder()
        .uri(format!("/api/web/audit/export?{query}"))
        .header(header::HOST, "web.example")
        .header(header::COOKIE, &f.admin)
        .body(Body::empty())
        .unwrap();
    let response = f.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (headers, String::from_utf8(bytes.to_vec()).unwrap())
}

#[sqlx::test(migrations = "./migrations")]
async fn successful_changes_capture_exact_values_ids_and_password_redaction(pool: PgPool) {
    let f = fixture(&pool).await;
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/web/accounts/{}/traffic", f.target))
                .header(header::HOST, "web.example")
                .header(header::ORIGIN, "https://web.example")
                .header(header::COOKIE, &f.admin)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-pixels-request-id", "untrusted-request-id")
                .body(Body::from(json!({"mbps":10}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let request_id: Uuid = response.headers()["x-pixels-request-id"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let update = call(
        &f.app,
        "PATCH",
        &format!("/api/web/accounts/{}", f.target),
        &f.admin,
        json!({"status":"active","server_admin":true,"revision":1}),
    )
    .await;
    assert_eq!(update.0, StatusCode::OK, "{}", update.2);
    let op = Uuid::new_v4();
    let password = "never serialize this password or its length";
    let body = json!({"operation_id":op,"revision":2,"new_password":password});
    let reset = call(
        &f.app,
        "POST",
        &format!("/api/web/accounts/{}/password-reset", f.target),
        &f.admin,
        body.clone(),
    )
    .await;
    assert_eq!(reset.0, StatusCode::OK, "{}", reset.2);
    assert_eq!(
        call(
            &f.app,
            "POST",
            &format!("/api/web/accounts/{}/password-reset", f.target),
            &f.admin,
            body
        )
        .await
        .0,
        StatusCode::OK
    );
    let data = list(
        &f,
        &format!(
            "actor={}&resource_kind=account&resource={}",
            f.actor, f.target
        ),
    )
    .await;
    let items = data["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    let bandwidth = items
        .iter()
        .find(|e| e["action"] == "account.relay_limit_updated")
        .unwrap();
    assert_eq!(bandwidth["request_id"], request_id.to_string());
    assert_eq!(bandwidth["after"], json!({"mbps":10}));
    assert!(bandwidth["before"]["mbps"].is_number());
    let status = items
        .iter()
        .find(|e| e["action"] == "account.updated")
        .unwrap();
    assert_eq!(
        status["before"],
        json!({"status":"active","server_admin":false,"revision":1})
    );
    assert_eq!(
        status["after"],
        json!({"status":"active","server_admin":true,"revision":2})
    );
    let reset = items
        .iter()
        .find(|e| e["action"] == "account.password_reset")
        .unwrap();
    assert_eq!(reset["operation_id"], op.to_string());
    assert_eq!(reset["after"]["password_changed"], true);
    let raw: String =
        sqlx::query_scalar("SELECT string_agg(row_to_json(e)::text,'') FROM web_admin_events e")
            .fetch_one(&pool)
            .await
            .unwrap();
    for secret in [
        password,
        "password_hash",
        "password_length",
        "token_hash",
        "access_token",
    ] {
        assert!(!raw.contains(secret), "leaked {secret}");
        assert!(!data.to_string().contains(secret));
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn audit_failure_rolls_back_business_and_failed_attempts_are_sampled(pool: PgPool) {
    let f = fixture(&pool).await;
    sqlx::raw_sql("CREATE FUNCTION reject_success_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.action='account.updated' AND NEW.result='succeeded' THEN RAISE EXCEPTION 'isolated audit fault'; END IF; RETURN NEW; END $$; CREATE TRIGGER audit_fault BEFORE INSERT ON web_admin_events FOR EACH ROW EXECUTE FUNCTION reject_success_audit();").execute(&pool).await.unwrap();
    let url = format!("/api/web/accounts/{}", f.target);
    for _ in 0..3 {
        assert_eq!(
            call(
                &f.app,
                "PATCH",
                &url,
                &f.admin,
                json!({"status":"disabled","server_admin":false,"revision":1})
            )
            .await
            .0,
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
    let row = sqlx::query("SELECT status,auth_revision FROM users WHERE id=$1")
        .bind(f.target)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("status"), "active");
    assert_eq!(row.get::<i64, _>("auth_revision"), 1);
    assert_eq!(
        list(&f, "action=account.updated&result=succeeded").await["total"],
        0
    );
    let failed = list(&f, "action=account.updated&result=failed").await;
    assert_eq!(failed["total"], 1);
    assert_eq!(failed["items"][0]["details"]["error_code"], "server_error");
    for _ in 0..12 {
        assert_eq!(
            call(
                &f.app,
                "PATCH",
                &url,
                &f.user,
                json!({"status":"disabled","server_admin":false,"revision":1})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        list(
            &f,
            &format!("actor={}&action=account.updated&result=denied", f.target)
        )
        .await["total"],
        1
    );
    sqlx::raw_sql("DROP TRIGGER audit_fault ON web_admin_events")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            &f.app,
            "PATCH",
            &url,
            &f.admin,
            json!({"status":"disabled","server_admin":false,"revision":1})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        list(&f, "action=account.updated&result=succeeded").await["total"],
        1
    );
    assert_eq!(
        list(&f, "action=account.updated&result=failed").await["total"],
        1
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn snapshots_survive_renames_deletion_and_legacy_values_are_not_invented(pool: PgPool) {
    let f = fixture(&pool).await;
    let endpoint = unclaimed_fixture(&pool).await;
    let pab_protocol::EndpointProofPrincipal::Device { device_id } = endpoint.principal else {
        panic!()
    };
    let path = format!("/api/web/devices/{device_id}");
    assert_eq!(
        call(
            &f.app,
            "PATCH",
            &path,
            &f.admin,
            json!({"name":"first captured name","revision":1})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &f.app,
            "PATCH",
            &path,
            &f.admin,
            json!({"name":"second current name","revision":2})
        )
        .await
        .0,
        StatusCode::OK
    );
    sqlx::query("UPDATE users SET username='current-admin-name' WHERE id=$1")
        .bind(f.actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM endpoints WHERE device_id=$1")
        .bind(device_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM devices WHERE id=$1")
        .bind(device_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO web_admin_events(actor_id,action,resource_id,details) VALUES($1,'legacy.fixture',$2,$3)").bind(f.actor).bind(f.target).bind(json!({"password":"historical secret must not be surfaced"})).execute(&pool).await.unwrap();
    let data = list(&f, "action=device.rename").await;
    let first = &data["items"][1];
    assert_eq!(first["actor"], "audit-admin");
    assert_eq!(first["resource_name"], "first captured name");
    assert_eq!(first["after"]["name"], "first captured name");
    let legacy = list(&f, "action=legacy.fixture").await;
    let row = &legacy["items"][0];
    assert_eq!(row["change_state"], "historical_unknown");
    for key in [
        "actor",
        "resource_name",
        "before",
        "after",
        "details",
        "request_id",
    ] {
        assert!(row[key].is_null(), "{key}");
    }
    let (_, csv) = csv(&f, "action=legacy.fixture").await;
    assert!(!csv.contains("historical secret"));
}

#[sqlx::test(migrations = "./migrations")]
async fn frozen_keyset_excludes_late_commits_without_skipping_or_duplicating_rows(pool: PgPool) {
    let f = fixture(&pool).await;
    let mut pending = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO web_admin_events(action) VALUES('fixture.seed')")
        .execute(&mut *pending)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO web_admin_events(action) SELECT 'fixture.seed' FROM generate_series(1,105)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let first = list(&f, "action=fixture.seed&page_size=20").await;
    assert_eq!(first["total"], 105);
    pending.commit().await.unwrap();
    sqlx::query("INSERT INTO web_admin_events(action) VALUES('fixture.seed')")
        .execute(&pool)
        .await
        .unwrap();
    let mut page = first.clone();
    let mut ids = Vec::new();
    loop {
        assert_eq!(page["total"], 105);
        ids.extend(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["id"].as_str().unwrap().to_owned()),
        );
        let Some(cursor) = page["next_cursor"].as_str() else {
            break;
        };
        page = list(
            &f,
            &format!("action=fixture.seed&page_size=20&cursor={cursor}"),
        )
        .await;
        if ids.len() == 20 {
            let previous = list(
                &f,
                &format!(
                    "action=fixture.seed&page_size=20&cursor={}",
                    page["previous_cursor"].as_str().unwrap()
                ),
            )
            .await;
            assert_eq!(previous["items"], first["items"]);
        }
    }
    assert_eq!(ids.len(), 105);
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), 105);
    let (headers, text) = csv(
        &f,
        &format!(
            "action=fixture.seed&page_size=20&cursor={}",
            first["snapshot_cursor"].as_str().unwrap()
        ),
    )
    .await;
    assert_eq!(headers["x-audit-rows"], "105");
    assert_eq!(
        text.lines().filter(|l| l.starts_with("\"data\"")).count(),
        105
    );
    assert_eq!(list(&f, "action=fixture.seed").await["total"], 107);
}

#[sqlx::test(migrations = "./migrations")]
async fn combined_filters_permissions_and_cursor_validation_are_enforced(pool: PgPool) {
    let f = fixture(&pool).await;
    sqlx::query("INSERT INTO web_admin_events(actor_id,actor_ref,actor_name,action,result,resource_kind,resource_key,resource_name,created_at,format_version) VALUES($1,$1,'Audit snapshot','fixture.filters','succeeded','account',$2,'Target snapshot',to_timestamp(1800000000),1)").bind(f.actor).bind(f.target.to_string()).execute(&pool).await.unwrap();
    let query = format!(
        "from=1800000000000&until=1800000000001&actor={}&action=fixture.filters&result=succeeded&resource_kind=account&resource={}&q=Snapshot",
        f.actor, f.target
    );
    let result = list(&f, &query).await;
    assert_eq!(result["total"], 1);
    assert_eq!(
        list(&f, &query.replace("result=succeeded", "result=failed")).await["total"],
        0
    );
    let changed = format!(
        "/api/web/audit?{query}&page_size=50&cursor={}",
        result["snapshot_cursor"].as_str().unwrap()
    );
    let rejected = call(&f.app, "GET", &changed, &f.admin, Value::Null).await;
    assert_eq!(rejected.0, StatusCode::BAD_REQUEST);
    assert_eq!(rejected.2["code"], "audit_cursor_mismatch");
    for query in [
        "cursor=nothex",
        "cursor=f0",
        "page=2",
        "page_size=101",
        "from=2&until=1",
        "actor=invalid",
        "result=unexpected",
        "resource_kind=token",
    ] {
        assert_eq!(
            call(
                &f.app,
                "GET",
                &format!("/api/web/audit?{query}"),
                &f.admin,
                Value::Null
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "{query}"
        );
    }
    for path in ["/api/web/audit", "/api/web/audit/export"] {
        assert_eq!(
            call(&f.app, "GET", path, &f.user, Value::Null).await.0,
            StatusCode::FORBIDDEN
        );
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn hundred_thousand_events_have_bounded_csv_safe_cells_and_explicit_truncation(pool: PgPool) {
    let f = fixture(&pool).await;
    sqlx::query("INSERT INTO web_admin_events(action,actor_name,resource_kind,resource_key,resource_name,format_version,after_values) SELECT 'fixture.volume','=unsafe spreadsheet','device',g::text,'测试设备',1,'{\"name\":\"测试设备\"}'::jsonb FROM generate_series(1,100000) g").execute(&pool).await.unwrap();
    let started = std::time::Instant::now();
    let result = list(&f, "action=fixture.volume&page_size=100").await;
    assert_eq!(result["total"], 100000);
    assert_eq!(result["items"].as_array().unwrap().len(), 100);
    assert!(started.elapsed().as_secs() < 10);
    let (headers, text) = csv(
        &f,
        &format!(
            "action=fixture.volume&page_size=100&cursor={}",
            result["snapshot_cursor"].as_str().unwrap()
        ),
    )
    .await;
    assert_eq!(headers["x-audit-truncated"], "true");
    assert_eq!(headers["x-audit-rows"], "10000");
    assert!(text.starts_with('\u{feff}'));
    assert_eq!(text.lines().count(), 10002);
    assert!(text.contains("\"'=unsafe spreadsheet\""));
    assert!(text.contains("测试设备"));
    assert!(text.lines().last().unwrap().ends_with("\"10000\",\"true\""));
    let audit = list(&f, "action=audit.exported").await;
    assert_eq!(audit["total"], 1);
    assert_eq!(
        audit["items"][0]["details"],
        json!({"rows":10000,"truncated":true,"filters_applied":true})
    );
    let retained: i64 =
        sqlx::query_scalar("SELECT count(*) FROM web_admin_events WHERE action='fixture.volume'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(retained, 100000);
    println!(
        "B5 100000-row query and 10000-row export: {:?}",
        started.elapsed()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn reload_audit_failure_keeps_database_and_published_configuration_unchanged(pool: PgPool) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("server.toml");
    let valid = include_str!("../../../../packaging/server/cluster.example.toml");
    std::fs::write(&path, valid).unwrap();
    let loaded = pab_server::config::ServerConfig::load(&path).unwrap();
    let control = ControlPlane::new(
        PostgresStore::from_pool(pool.clone()),
        PasswordPolicy::default(),
    )
    .unwrap();
    control.initialize_settings().await.unwrap();
    let control = control
        .with_relay_cluster(loaded.relay.cluster.clone())
        .await
        .unwrap()
        .with_relay_config_source(path.clone(), &loaded.relay.control_secret);
    let state = ControlApiState::new(
        control.clone(),
        ControlApiConfig {
            registration_enabled: true,
            ..Default::default()
        },
        RelayControlAuth::new(&loaded.relay.control_secret).unwrap(),
    );
    let app = pab_server::control_router(state);
    let (_, cookie, _) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"reload-audit-admin","password":"isolated reload audit password"}),
    )
    .await;
    pab_server::web::bootstrap_admin(&control, "reload-audit-admin")
        .await
        .unwrap();
    let before = control.store().relay_directory(15).await.unwrap();
    std::fs::write(&path, valid.replace("weight = 100", "weight = 90")).unwrap();
    sqlx::raw_sql("CREATE FUNCTION reject_reload_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.action='relay_config_reloaded' AND NEW.result='succeeded' THEN RAISE EXCEPTION 'isolated reload audit fault'; END IF; RETURN NEW; END $$; CREATE TRIGGER reload_audit_fault BEFORE INSERT ON web_admin_events FOR EACH ROW EXECUTE FUNCTION reject_reload_audit();").execute(&pool).await.unwrap();
    assert_eq!(
        call(&app, "POST", "/api/web/relays/reload", &cookie, json!({}))
            .await
            .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(control.store().relay_directory(15).await.unwrap(), before);
    sqlx::raw_sql("DROP TRIGGER reload_audit_fault ON web_admin_events")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(&app, "POST", "/api/web/relays/reload", &cookie, json!({}))
            .await
            .0,
        StatusCode::OK
    );
    let audit = call(
        &app,
        "GET",
        "/api/web/audit?action=relay_config_reloaded&result=succeeded",
        &cookie,
        Value::Null,
    )
    .await;
    assert_eq!(audit.0, StatusCode::OK, "{}", audit.2);
    assert_eq!(audit.2["total"], 1);
    assert_eq!(audit.2["items"][0]["before"]["nodes"][0]["weight"], 100);
    assert_eq!(audit.2["items"][0]["after"]["nodes"][0]["weight"], 90);
    for node in loaded.relay.cluster.unwrap().nodes {
        assert!(!audit.2.to_string().contains(&node.control_secret));
    }
    let snapshot = call(&app, "GET", "/api/web/relays", &cookie, Value::Null)
        .await
        .2;
    let instance: Uuid = snapshot["server_instance"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    sqlx::query("UPDATE relay_cluster_nodes SET server_instance=$1,health='healthy',last_seen_at=clock_timestamp()")
        .bind(instance).execute(&pool).await.unwrap();
    for (desired, previous, action) in [
        ("maintenance", "active", "relay_maintenance"),
        ("disabled", "maintenance", "relay_disable"),
        ("active", "disabled", "relay_activate"),
    ] {
        let operation = Uuid::new_v4();
        let body = json!({"state":desired,"operation_id":operation,"allow_outage":true});
        for _ in 0..2 {
            let changed = call(
                &app,
                "PATCH",
                "/api/web/relays/linux-a/state",
                &cookie,
                body.clone(),
            )
            .await;
            assert_eq!(changed.0, StatusCode::OK, "{}", changed.2);
        }
        let data = call(
            &app,
            "GET",
            &format!("/api/web/audit?action={action}&result=succeeded"),
            &cookie,
            Value::Null,
        )
        .await
        .2;
        assert_eq!(data["total"], 1);
        let event = &data["items"][0];
        assert_eq!(event["before"], json!({"status":previous}));
        assert_eq!(event["after"], json!({"status":desired}));
        assert_eq!(event["operation_id"], operation.to_string());
        assert_eq!(event["resource_key"], "linux-a");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn self_password_session_revocation_and_usage_export_have_typed_audits(pool: PgPool) {
    let f = fixture(&pool).await;
    let changed=call(&f.app,"POST","/api/web/password",&f.user,json!({"current_password":"isolated audit fixture password","new_password":"isolated self audit replacement"})).await;
    assert_eq!(changed.0, StatusCode::NO_CONTENT);
    let event = list(&f, "action=account.password_changed").await;
    assert_eq!(event["total"], 1);
    assert_eq!(event["items"][0]["after"], json!({"password_changed":true}));
    let operation = Uuid::new_v4();
    let body = json!({"operation_id":operation,"scope":"all"});
    for _ in 0..2 {
        assert_eq!(
            call(
                &f.app,
                "POST",
                &format!("/api/web/accounts/{}/sessions/revoke", f.target),
                &f.admin,
                body.clone()
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let event = list(&f, "action=account.sessions_revoked").await;
    assert_eq!(event["total"], 1);
    assert_eq!(event["items"][0]["after"]["sessions_revoked"], 1);
    assert_eq!(event["items"][0]["operation_id"], operation.to_string());
    assert_eq!(
        call(
            &f.app,
            "GET",
            "/api/web/usage/export?interval=lifetime",
            &f.admin,
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    let event = list(&f, "action=usage.exported").await;
    assert_eq!(event["total"], 1);
    assert_eq!(event["items"][0]["details"]["interval"], "lifetime");
    let boot = list(&f, "action=admin.bootstrap").await;
    assert_eq!(boot["total"], 1);
    assert_eq!(boot["items"][0]["before"]["server_admin"], false);
    assert_eq!(boot["items"][0]["after"]["server_admin"], true);
}
