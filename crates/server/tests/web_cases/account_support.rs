use super::*;
use uuid::Uuid;

struct Fixture {
    app: Router,
    control: ControlPlane,
    admin: String,
    admin_id: Uuid,
    user: String,
    user_id: Uuid,
}
async fn fixture(pool: PgPool) -> Fixture {
    let (app, control) = app(pool, true).await;
    let (_, admin, admin_account) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"support-admin","password":"support fixture original password"}),
    )
    .await;
    let (_, user, user_account) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"support-user","password":"support fixture original password"}),
    )
    .await;
    pab_server::web::bootstrap_admin(&control, "support-admin")
        .await
        .unwrap();
    Fixture {
        app,
        control,
        admin,
        user,
        admin_id: Uuid::parse_str(admin_account["id"].as_str().unwrap()).unwrap(),
        user_id: Uuid::parse_str(user_account["id"].as_str().unwrap()).unwrap(),
    }
}
fn token(cookie: &str) -> &str {
    cookie.split(';').next().unwrap().split_once('=').unwrap().1
}
async fn native_login(f: &Fixture) -> String {
    let (status, result) = native_call(
        &f.app,
        "POST",
        "/api/account/session",
        "",
        json!({"username":"support-user","password":"support fixture original password"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    result["access_token"].as_str().unwrap().to_owned()
}

#[sqlx::test(migrations = "./migrations")]
async fn ordinary_accounts_cannot_read_or_mutate_other_account_support(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let (_, other, b) = call(
        &f.app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"support-other","password":"support fixture original password"}),
    )
    .await;
    let id = b["id"].as_str().unwrap();
    for path in [
        format!("/api/web/accounts/{id}"),
        format!("/api/web/accounts/{id}/sessions"),
        format!("/api/web/devices?user={id}"),
        format!("/api/web/usage?user={id}"),
    ] {
        assert_eq!(
            call(&f.app, "GET", &path, &f.user, Value::Null).await.0,
            StatusCode::FORBIDDEN
        );
    }
    for (path, body) in [
        (
            format!("/api/web/accounts/{id}/password-reset"),
            json!({"operation_id":Uuid::new_v4(),"revision":1,"new_password":"never applied password"}),
        ),
        (
            format!("/api/web/accounts/{id}/sessions/revoke"),
            json!({"operation_id":Uuid::new_v4(),"scope":"all"}),
        ),
    ] {
        assert_eq!(
            call(&f.app, "POST", &path, &f.user, body).await.0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        call(&f.app, "GET", "/api/web/session", &other, Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM account_support_operations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn account_detail_devices_and_exact_usage_agree_without_control_grants(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let endpoint = unclaimed_fixture(&pool).await;
    let pab_protocol::EndpointProofPrincipal::Device { device_id } = endpoint.principal else {
        panic!()
    };
    sqlx::query("INSERT INTO device_accounts(device_id,user_id,revision) VALUES($1,$2,1)")
        .bind(device_id.as_uuid())
        .bind(f.user_id)
        .execute(&pool)
        .await
        .unwrap();
    let hour = (time::OffsetDateTime::now_utc().unix_timestamp() / 3600) * 3600000;
    let counters = pab_protocol::UsageCounters {
        p2p_reported: true,
        p2p_upload_bytes: 1234,
        p2p_download_bytes: 5678,
        uploaded_bytes: 9007199254740993,
        connections: 7,
        incomplete: true,
        ..Default::default()
    };
    sqlx::query("INSERT INTO usage_hourly(source_kind,subject,hour_unix_ms,counters) VALUES('client',$1,$2,$3)").bind(f.user_id.to_string()).bind(hour).bind(serde_json::to_value(counters).unwrap()).execute(&pool).await.unwrap();
    let (status, _, detail) = call(
        &f.app,
        "GET",
        &format!("/api/web/accounts/{}", f.user_id),
        &f.admin,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["device_count"], 1);
    assert_eq!(
        detail["usage"]["counters"]["uploaded_bytes"],
        "9007199254740993"
    );
    assert_eq!(detail["usage"]["counters"]["connections"], "7");
    assert_eq!(detail["usage"]["incomplete"], true);
    assert_eq!(detail["usage"]["counters"]["p2p_upload_bytes"], "1234");
    assert_eq!(detail["usage"]["counters"]["p2p_download_bytes"], "5678");
    assert!(detail["usage"]["counters"].get("p2p_reported").is_none());
    let (status, _, devices) = call(
        &f.app,
        "GET",
        &format!("/api/web/devices?scope=all&user={}", f.user_id),
        &f.admin,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(devices["total"], 1);
    assert_eq!(devices["items"][0]["associated_with_me"], false);
    let (_, _, usage) = call(
        &f.app,
        "GET",
        &format!(
            "/api/web/usage?scope=all&user={}&from={}&until={}",
            f.user_id, detail["usage"]["from"], detail["usage"]["until"]
        ),
        &f.admin,
        Value::Null,
    )
    .await;
    assert_eq!(usage["counters"]["uploaded_bytes"], 9007199254740993u64);
    assert!(!detail.to_string().contains("password_hash"));
    let tenant: Option<Uuid> =
        sqlx::query_scalar("SELECT owner_tenant_id FROM devices WHERE id=$1")
            .bind(device_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(tenant.is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn admin_added_devices_include_removed_and_expired_entries_with_account_isolation(
    pool: PgPool,
) {
    let f = fixture(pool.clone()).await;
    let endpoint = unclaimed_fixture(&pool).await;
    sqlx::query("INSERT INTO devices(id,tenant_id,code,name) SELECT gen_random_uuid(),$1,720000000+i,'Added '||i FROM generate_series(1,25) i")
        .bind(endpoint.tenant_id.as_uuid()).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO user_saved_devices(user_id,device_id,revision,saved_name,saved_system,alias,deleted,verified_until) SELECT $1,id,code-720000000,name,'windows','Personal note',code%2=0,to_timestamp(0) FROM devices WHERE code BETWEEN 720000001 AND 720000025")
        .bind(f.user_id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO user_saved_devices(user_id,device_id,revision,saved_name,alias,verified_until) SELECT $1,id,1,'Only admin saved name','only-admin-alias',to_timestamp(0) FROM devices WHERE code=720000001")
        .bind(f.admin_id).execute(&pool).await.unwrap();
    let accounts = call(&f.app, "GET", "/api/web/accounts", &f.admin, Value::Null)
        .await
        .2;
    let user = accounts["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == f.user_id.to_string())
        .unwrap();
    assert_eq!(user["saved_device_count"], 25);
    assert_eq!(user["device_count"], 0);
    let path = format!("/api/web/accounts/{}/saved-devices", f.user_id);
    let mut seen = std::collections::HashSet::new();
    let mut removed = 0;
    for (page, size) in [(1, 20), (2, 5)] {
        let result = call(
            &f.app,
            "GET",
            &format!("{path}?page={page}&page_size=20"),
            &f.admin,
            Value::Null,
        )
        .await;
        assert_eq!(result.0, StatusCode::OK, "{}", result.2);
        assert_eq!(result.2["total"], 25);
        let items = result.2["items"].as_array().unwrap();
        assert_eq!(items.len(), size);
        for item in items {
            assert!(seen.insert(item["id"].as_str().unwrap().to_owned()));
            assert_eq!(item["alias"], "Personal note");
            assert!(item["created_at"].is_number());
            removed += usize::from(item["deleted"] == true);
        }
        assert!(!result.2.to_string().contains("only-admin-alias"));
        assert!(!result.2.to_string().contains("verified_until"));
    }
    assert_eq!(seen.len(), 25);
    assert_eq!(removed, 12);
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("{path}?q=only-admin-alias"),
            &f.admin,
            Value::Null
        )
        .await
        .2["total"],
        0
    );
    for cookie in [&f.user, ""] {
        assert_eq!(
            call(&f.app, "GET", &path, cookie, Value::Null).await.0,
            if cookie.is_empty() {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::FORBIDDEN
            }
        );
    }
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("/api/web/accounts/{}/saved-devices", Uuid::new_v4()),
            &f.admin,
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("{path}?page_size=101"),
            &f.admin,
            Value::Null
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn password_reset_retries_return_first_result_and_revoke_only_target_sessions(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let native = native_login(&f).await;
    let id = Uuid::new_v4();
    let path = format!("/api/web/accounts/{}/password-reset", f.user_id);
    let (status, _, first) = call(
        &f.app,
        "POST",
        &path,
        &f.admin,
        json!({"operation_id":id,"revision":1,"new_password":"first committed replacement"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["sessions_revoked"], 2);
    assert_eq!(first["propagation"], "pending");
    let (a, b) = tokio::join!(
        call(
            &f.app,
            "POST",
            &path,
            &f.admin,
            json!({"operation_id":id,"revision":999,"new_password":"ignored replacement password"})
        ),
        call(
            &f.app,
            "POST",
            &path,
            &f.admin,
            json!({"operation_id":id,"revision":0})
        ),
    );
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(b.0, StatusCode::OK);
    assert_eq!(a.2, first);
    assert_eq!(b.2, first);
    assert_eq!(
        call(&f.app, "GET", "/api/web/session", &f.user, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        native_call(&f.app, "GET", "/api/account/session", &native, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&f.app, "GET", "/api/web/session", &f.admin, Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &f.app,
            "POST",
            "/api/web/session",
            "",
            json!({"username":"support-user","password":"first committed replacement"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &f.app,
            "POST",
            "/api/web/session",
            "",
            json!({"username":"support-user","password":"ignored replacement password"})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("/api/web/account-operations/{id}"),
            &f.admin,
            Value::Null
        )
        .await
        .2,
        first
    );
    let row: Value =
        sqlx::query_scalar("SELECT to_jsonb(o) FROM account_support_operations o WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let audit: Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(e)) FROM web_admin_events e WHERE operation_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit.as_array().unwrap().len(), 1);
    assert_eq!(audit[0]["result"], "succeeded");
    for secret in [
        "first committed replacement",
        "ignored replacement password",
        native.as_str(),
        token(&f.admin),
        token(&f.user),
    ] {
        assert!(!format!("{first}{row}{audit}").contains(secret));
    }
    assert!(row.get("password_hash").is_none());
    assert!(row.get("digest").is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn recovery_preserves_disabled_role_and_rejects_github_only_or_self_reset(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let path = format!("/api/web/accounts/{}/password-reset", f.user_id);
    sqlx::query("UPDATE users SET status='disabled',server_admin=true,auth_revision=auth_revision+1 WHERE id=$1").bind(f.user_id).execute(&pool).await.unwrap();
    let (status,_,result)=call(&f.app,"POST",&path,&f.admin,json!({"operation_id":Uuid::new_v4(),"revision":2,"new_password":"disabled replacement password"})).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let (_, _, detail) = call(
        &f.app,
        "GET",
        &format!("/api/web/accounts/{}", f.user_id),
        &f.admin,
        Value::Null,
    )
    .await;
    assert_eq!(detail["status"], "disabled");
    assert_eq!(detail["server_admin"], true);
    assert_eq!(
        call(
            &f.app,
            "POST",
            "/api/web/session",
            "",
            json!({"username":"support-user","password":"disabled replacement password"})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE users SET password_hash=NULL WHERE id=$1")
        .bind(f.user_id)
        .execute(&pool)
        .await
        .unwrap();
    let (status,_,result)=call(&f.app,"POST",&path,&f.admin,json!({"operation_id":Uuid::new_v4(),"revision":3,"new_password":"must not create password"})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(result["code"], "password_login_required");
    let (status,_,result)=call(&f.app,"POST",&format!("/api/web/accounts/{}/password-reset",f.admin_id),&f.admin,json!({"operation_id":Uuid::new_v4(),"revision":1,"new_password":"must not reset self password"})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(result["code"], "use_self_password_change");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM account_support_operations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn sessions_have_opaque_ids_and_others_revocation_keeps_current_permanent_login(
    pool: PgPool,
) {
    let f = fixture(pool.clone()).await;
    let native = native_login(&f).await;
    let (_, other, _) = call(
        &f.app,
        "POST",
        "/api/web/session",
        "",
        json!({"username":"support-user","password":"support fixture original password"}),
    )
    .await;
    sqlx::query("INSERT INTO web_sessions(token_hash,user_id,created_at) VALUES(repeat('a',64),$1,now()-interval '10 years')").bind(f.user_id).execute(&pool).await.unwrap();
    let (status, _, sessions) =
        call(&f.app, "GET", "/api/web/sessions", &f.user, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{sessions}");
    assert_eq!(sessions["total"], 4);
    let items = sessions["items"].as_array().unwrap();
    assert_eq!(items.iter().filter(|s| s["current"] == true).count(), 1);
    assert!(items.iter().any(|s| s["channel"] == "native"));
    assert!(
        items
            .iter()
            .any(|s| s["channel"] == "unknown" && s["last_active_at"].is_null())
    );
    assert!(
        items
            .iter()
            .all(|s| Uuid::parse_str(s["id"].as_str().unwrap()).is_ok())
    );
    assert!(!sessions.to_string().contains("token_hash"));
    assert!(!sessions.to_string().contains(&native));
    let id = Uuid::new_v4();
    let input = json!({"operation_id":id,"scope":"others"});
    let (status, _, result) = call(
        &f.app,
        "POST",
        "/api/web/sessions/revoke",
        &f.user,
        input.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["sessions_revoked"], 3);
    assert_eq!(
        call(&f.app, "POST", "/api/web/sessions/revoke", &f.user, input)
            .await
            .2,
        result
    );
    assert_eq!(
        call(&f.app, "GET", "/api/web/session", &f.user, Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&f.app, "GET", "/api/web/session", &other, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        native_call(&f.app, "GET", "/api/account/session", &native, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &f.app,
            "POST",
            "/api/web/sessions/revoke",
            &f.user,
            json!({"operation_id":id,"scope":"all"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &f.app,
            "POST",
            "/api/web/sessions/revoke",
            &f.user,
            json!({"operation_id":Uuid::new_v4(),"scope":"all"})
        )
        .await
        .2["sessions_revoked"],
        1
    );
    assert_eq!(
        call(&f.app, "GET", "/api/web/session", &f.user, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn single_session_and_optional_password_preservation_are_revision_checked(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let native = native_login(&f).await;
    let session_id: Uuid = sqlx::query_scalar("SELECT id FROM web_sessions WHERE token_hash=$1")
        .bind(blake3::hash(native.as_bytes()).to_hex().to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
    let path = format!("/api/web/accounts/{}/sessions/revoke", f.user_id);
    let (status, _, result) = call(
        &f.app,
        "POST",
        &path,
        &f.admin,
        json!({"operation_id":Uuid::new_v4(),"scope":"session","session_id":session_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["sessions_revoked"], 1);
    assert_eq!(
        call(&f.app, "GET", "/api/web/session", &f.user, Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    let path = format!("/api/web/accounts/{}/password-reset", f.user_id);
    assert_eq!(call(&f.app,"POST",&path,&f.admin,json!({"operation_id":Uuid::new_v4(),"revision":0,"new_password":"stale password replacement"})).await.0,StatusCode::CONFLICT);
    assert_eq!(call(&f.app,"POST",&path,&f.admin,json!({"operation_id":Uuid::new_v4(),"revision":1,"new_password":"keep sessions replacement","revoke_sessions":false})).await.0,StatusCode::OK);
    assert_eq!(
        call(&f.app, "GET", "/api/web/session", &f.user, Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    let active: Option<time::OffsetDateTime> =
        sqlx::query_scalar("SELECT last_active_at FROM web_sessions WHERE user_id=$1")
            .bind(f.user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    call(&f.app, "GET", "/api/web/session", &f.user, Value::Null).await;
    let unchanged: Option<time::OffsetDateTime> =
        sqlx::query_scalar("SELECT last_active_at FROM web_sessions WHERE user_id=$1")
            .bind(f.user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(active, unchanged);
}

#[sqlx::test(migrations = "./migrations")]
async fn revoked_websocket_closes_within_fifteen_seconds_and_other_session_stays_live(
    pool: PgPool,
) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{Message, client::IntoClientRequest},
    };
    let f = fixture(pool.clone()).await;
    let (_, other, _) = call(
        &f.app,
        "POST",
        "/api/web/session",
        "",
        json!({"username":"support-user","password":"support fixture original password"}),
    )
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = f.app.clone();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let request = |cookie: &str| {
        let mut r = format!("ws://{address}/api/web/events")
            .into_client_request()
            .unwrap();
        r.headers_mut()
            .insert("origin", format!("https://{address}").parse().unwrap());
        r.headers_mut()
            .insert("cookie", cookie.split(';').next().unwrap().parse().unwrap());
        r
    };
    let (mut revoked, _) = connect_async(request(&f.user)).await.unwrap();
    let (mut kept, _) = connect_async(request(&other)).await.unwrap();
    revoked.next().await.unwrap().unwrap();
    kept.next().await.unwrap().unwrap();
    let session_id: Uuid = sqlx::query_scalar("SELECT id FROM web_sessions WHERE token_hash=$1")
        .bind(blake3::hash(token(&f.user).as_bytes()).to_hex().to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
    let start = std::time::Instant::now();
    let (status, _, _) = call(
        &f.app,
        "POST",
        &format!("/api/web/accounts/{}/sessions/revoke", f.user_id),
        &f.admin,
        json!({"operation_id":Uuid::new_v4(),"scope":"session","session_id":session_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        call(&f.app, "GET", "/api/web/session", &f.user, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while let Some(message) = revoked.next().await {
            if matches!(message.unwrap(), Message::Close(_)) {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(15));
    eprintln!(
        "single-session websocket invalidation elapsed_ms={}",
        start.elapsed().as_millis()
    );
    kept.send(Message::Ping(vec![1].into())).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match kept.next().await.unwrap().unwrap() {
                Message::Pong(_) => break,
                Message::Text(_) => {}
                other => panic!("other session unexpectedly closed: {other:?}"),
            }
        }
    })
    .await
    .unwrap();
    drop(kept);
    drop(revoked);
    server.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn revoked_native_context_is_rejected_on_a_live_wss_control_channel(pool: PgPool) {
    use axum_server::tls_rustls::RustlsConfig;
    use iroh_base::SecretKey;
    use pab_agent_core::{
        AuthenticatedControlConnection, ControlConnectionPhase, EndpointControlConfig,
        EndpointControlError, EndpointControlSupervisor, PeerAuthorizationError, ReconnectPolicy,
        tls_connector,
    };
    use pab_protocol::{ControlErrorCode, EndpointKey, EndpointProofPrincipal, TenantId, UserId};
    use std::time::{Duration, Instant};

    let _ = rustls::crypto::ring::default_provider().install_default();
    let f = fixture(pool.clone()).await;
    let native = native_login(&f).await;
    let native_hash = blake3::hash(native.as_bytes()).to_hex().to_string();
    let target = unclaimed_fixture(&pool).await;
    let device_secret = SecretKey::from_bytes(&[201; 32]);
    sqlx::query("UPDATE endpoints SET endpoint_key=$1 WHERE endpoint_key=$2")
        .bind(device_secret.public().as_bytes().as_slice())
        .bind(target.endpoint_key.as_bytes().as_slice())
        .execute(&pool)
        .await
        .unwrap();
    let operator_secret = SecretKey::from_bytes(&[202; 32]);
    let operator_key = EndpointKey::new(*operator_secret.public().as_bytes());
    let tenant: Uuid =
        sqlx::query_scalar("SELECT tenant_id FROM personal_tenants WHERE user_id=$1")
            .bind(f.user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO endpoints(endpoint_key,tenant_id,owner_kind,user_id) VALUES($1,$2,'user',$3)",
    )
    .bind(operator_key.as_bytes().as_slice())
    .bind(tenant)
    .bind(f.user_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO endpoint_user_contexts(endpoint_key,token_hash,revision) VALUES($1,$2,1)",
    )
    .bind(operator_key.as_bytes().as_slice())
    .bind(&native_hash)
    .execute(&pool)
    .await
    .unwrap();
    let EndpointProofPrincipal::Device { device_id } = target.principal else {
        panic!("expected device")
    };
    sqlx::query("INSERT INTO device_connection_intents(operator_endpoint_key,device_id,expires_at) VALUES($1,$2,now()+interval '5 minutes')")
        .bind(operator_key.as_bytes().as_slice()).bind(device_id.as_uuid()).execute(&pool).await.unwrap();
    let certified = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let tls = RustlsConfig::from_der(
        vec![certified.cert.der().to_vec()],
        certified.signing_key.serialize_der(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = axum_server::Handle::new();
    let server_handle = handle.clone();
    let state = ControlApiState::new(
        f.control.clone(),
        ControlApiConfig::default(),
        RelayControlAuth::new("isolated account support control test secret").unwrap(),
    );
    let server = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .unwrap()
            .handle(server_handle)
            .serve(pab_server::control_router(state).into_make_service())
            .await
            .unwrap();
    });
    let connector = tls_connector(Some(certified.cert.pem().as_bytes())).unwrap();
    let operator_config = EndpointControlConfig {
        url: format!("wss://127.0.0.1:{port}/control"),
        tenant_id: TenantId::from_uuid(tenant),
        principal: EndpointProofPrincipal::User {
            user_id: UserId::from_uuid(f.user_id),
        },
        operation_timeout: Duration::from_secs(5),
    };
    let operator = AuthenticatedControlConnection::connect(
        &operator_config,
        &operator_secret,
        connector.clone(),
    )
    .await
    .unwrap();
    let device = EndpointControlSupervisor::new(
        EndpointControlConfig {
            url: operator_config.url.clone(),
            tenant_id: target.tenant_id,
            principal: target.principal,
            operation_timeout: Duration::from_secs(2),
        },
        device_secret,
        connector,
        ReconnectPolicy {
            heartbeat_interval: Duration::from_secs(1),
            heartbeat_timeout: Duration::from_millis(800),
            retry_interval: Duration::from_millis(100),
        },
    )
    .unwrap()
    .spawn();
    let authorizer = device.peer_authorizer();
    let status = device.status();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if status.borrow().phase == ControlConnectionPhase::Authenticated {
                let context = authorizer.authorize(operator_key).await.unwrap();
                assert!(context.user_context.user.is_some());
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let session_id: Uuid = sqlx::query_scalar("SELECT id FROM web_sessions WHERE token_hash=$1")
        .bind(native_hash)
        .fetch_one(&pool)
        .await
        .unwrap();
    let started = Instant::now();
    assert_eq!(
        call(
            &f.app,
            "POST",
            &format!("/api/web/accounts/{}/sessions/revoke", f.user_id),
            &f.admin,
            json!({"operation_id":Uuid::new_v4(),"scope":"session","session_id":session_id})
        )
        .await
        .0,
        StatusCode::OK
    );
    let error = tokio::time::timeout(Duration::from_secs(15), authorizer.authorize(operator_key))
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        matches!(error, PeerAuthorizationError::Control(EndpointControlError::Server { code: ControlErrorCode::PermissionDenied, ref message }) if message.starts_with("login required:")),
        "unexpected rejection: {error:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(15));
    eprintln!(
        "live WSS context invalidation elapsed_ms={}",
        started.elapsed().as_millis()
    );
    assert_eq!(status.borrow().phase, ControlConnectionPhase::Authenticated);
    assert_eq!(
        native_call(&f.app, "GET", "/api/account/session", &native, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&f.app, "GET", "/api/web/session", &f.user, Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    device.shutdown().await.unwrap();
    operator.close().await.unwrap();
    handle.shutdown();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}
