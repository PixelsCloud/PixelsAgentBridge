use super::*;
use crate::{
    ControlApiConfig, ControlApiState, ControlPlane, PasswordPolicy, PostgresStore,
    RelayControlAuth,
};
use axum::{
    Router,
    body::Body,
    http::Request,
    routing::{get, post},
};
use http_body_util::BodyExt;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tower::ServiceExt;

struct Fixture {
    app: Router,
    state: WebState,
    profile: Arc<Mutex<Value>>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
async fn fixture(pool: PgPool, registration: bool) -> Fixture {
    let profile = Arc::new(Mutex::new(
        json!({"id":1177,"login":"fixture-person","avatar_url":null}),
    ));
    let profile_handler = profile.clone();
    let mock=Router::new().route("/token",post(|axum::Form(form):axum::Form<std::collections::HashMap<String,String>>|async move{
        assert_eq!(form.get("client_id").unwrap(),"example-client");
        assert_eq!(form.get("client_secret").unwrap(),"example-secret-not-a-real-credential");
        if form.get("code").map(String::as_str)!=Some("good") || !form.get("code_verifier").is_some_and(|v|v.len()>=43){return (StatusCode::BAD_REQUEST,Json(json!({"error":"invalid_grant"}))).into_response()}
        Json(json!({"access_token":"fixture-token","token_type":"bearer","scope":""})).into_response()
    })).route("/user",get(move|headers:HeaderMap|{let profile=profile_handler.clone();async move{
        assert_eq!(headers.get(header::AUTHORIZATION).unwrap(),"Bearer fixture-token");Json(profile.lock().await.clone())
    }}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let cfg = GithubConfig::new(
        "example-client".into(),
        "example-secret-not-a-real-credential".into(),
        "https://pab.example".into(),
        "https://github.com/login/oauth/authorize",
        &format!("{origin}/token"),
        &format!("{origin}/user"),
    )
    .unwrap();
    let control =
        ControlPlane::new(PostgresStore::from_pool(pool), PasswordPolicy::default()).unwrap();
    control
        .initialize_settings(pab_protocol::RelayLimitDefaults {
            user_mbps: 5,
            guest_mbps: 1,
        })
        .await
        .unwrap();
    let api = ControlApiState::new(
        control.clone(),
        ControlApiConfig {
            registration_enabled: registration,
            ..Default::default()
        },
        RelayControlAuth::new("fixture-relay-control-secret-32-chars").unwrap(),
    )
    .with_github(Some(cfg));
    let state = WebState {
        control: Arc::new(control),
        registration_enabled: registration,
        github: api.github.clone(),
        login_slots: Arc::new(Semaphore::new(4)),
        login_budget: Arc::new(Mutex::new((std::time::Instant::now(), 0))),
        subscriptions: Arc::new(Semaphore::new(1)),
        server_instance: Uuid::new_v4(),
    };
    Fixture {
        app: crate::web::router(api),
        state,
        profile,
        server,
    }
}
async fn call(
    f: &Fixture,
    method: &str,
    path: &str,
    cookie: &str,
    bearer: &str,
    body: Value,
) -> (StatusCode, HeaderMap, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "pab.example")
        .header(header::ORIGIN, "https://pab.example")
        .header(header::CONTENT_TYPE, "application/json");
    if !cookie.is_empty() {
        req = req.header(header::COOKIE, cookie)
    }
    if !bearer.is_empty() {
        req = req.header(header::AUTHORIZATION, format!("Bearer {bearer}"))
    }
    let response = f
        .app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        serde_json::from_slice(&body).unwrap_or(Value::Null),
    )
}
fn cookies(h: &HeaderMap) -> String {
    h.get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().split(';').next().unwrap())
        .collect::<Vec<_>>()
        .join("; ")
}
fn location(h: &HeaderMap) -> String {
    h.get(header::LOCATION).unwrap().to_str().unwrap().into()
}
fn query(uri: &str, key: &str) -> String {
    url::Url::parse(uri)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == key)
        .unwrap()
        .1
        .into_owned()
}
async fn begin(f: &Fixture, session: &str, bind: bool) -> (String, String) {
    let (status, headers, value) = call(
        f,
        "POST",
        "/api/web/github/start",
        session,
        "",
        json!({"bind":bind}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let url = value["authorization_url"].as_str().unwrap();
    assert_eq!(query(url, "code_challenge_method"), "S256");
    assert!(!url.contains("example-secret"));
    (query(url, "state"), cookies(&headers))
}
async fn finish(
    f: &Fixture,
    state: &str,
    cookie: &str,
    code: &str,
) -> (StatusCode, HeaderMap, Value) {
    call(
        f,
        "GET",
        &format!("{CALLBACK}?state={state}&code={code}"),
        cookie,
        "",
        Value::Null,
    )
    .await
}
async fn password_user(f: &Fixture, name: &str) -> (Uuid, String) {
    let a = f
        .state
        .control
        .register_account(name, "fixture-password-123")
        .await
        .unwrap();
    let id = a.id.as_uuid();
    let token = session::issue_token(&f.state, id, 1).await.unwrap();
    (id, token)
}

#[test]
fn native_redirect_and_cookie_boundary() {
    assert!(random().len()>=32);
    assert!(valid_loopback("http://127.0.0.1:49152/github/callback"));
    for bad in [
        "http://localhost:49152/github/callback",
        "http://127.0.0.1.evil:49152/github/callback",
        "http://127.0.0.1:80/github/callback",
        "https://127.0.0.1:49152/github/callback",
        "http://127.0.0.1:49152/github/callback?next=evil",
        "http://user@127.0.0.1:49152/github/callback",
        "http://127.0.0.1:49152/other",
    ] {
        assert!(!valid_loopback(bad), "{bad}");
    }
    let mut h = HeaderMap::new();
    h.insert(
        header::COOKIE,
        format!("{COOKIE}={}; {COOKIE}={}", "a".repeat(43), "b".repeat(43))
            .parse()
            .unwrap(),
    );
    assert!(cookie(&h).is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn redeem_expiry_disabled_user_and_session_revision_are_enforced(pool:PgPool) {
    let f=fixture(pool.clone(),true).await;let(user,_)=password_user(&f,"redemption-boundary").await;
    let verifier="a".repeat(64);
    for (index,mode) in ["expired","disabled","changed"].into_iter().enumerate(){
        let code=format!("fixture-code-{index}");
        sqlx::query("UPDATE users SET status='active',auth_revision=1 WHERE id=$1").bind(user).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO github_redemptions(code_hash,proof_hash,user_id,auth_revision,expires_at) VALUES ($1,$2,$3,1,now()+interval '1 minute')")
            .bind(hash(&code)).bind(hash(&verifier)).bind(user).execute(&pool).await.unwrap();
        match mode{
            "expired"=>{sqlx::query("UPDATE github_redemptions SET expires_at=now()-interval '1 second'").execute(&pool).await.unwrap();},
            "disabled"=>{sqlx::query("UPDATE users SET status='disabled' WHERE id=$1").bind(user).execute(&pool).await.unwrap();},
            _=>{sqlx::query("UPDATE users SET auth_revision=2 WHERE id=$1").bind(user).execute(&pool).await.unwrap();},
        }
        let status=call(&f,"POST","/api/account/github/redeem","","",json!({"code":code,"verifier":verifier})).await.0;
        assert_eq!(status,if mode=="expired"{StatusCode::BAD_REQUEST}else{StatusCode::UNAUTHORIZED});
    }
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM web_sessions").fetch_one(&pool).await.unwrap(),1);
}

#[sqlx::test(migrations = "./migrations")]
async fn web_identity_is_stable_passwordless_and_does_not_merge_names(pool: PgPool) {
    let f = fixture(pool.clone(), true).await;
    let (local, _) = password_user(&f, "fixture-person").await;
    let (state, csrf) = begin(&f, "", false).await;
    let (status, h, _) = finish(&f, &state, &csrf, "good").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location(&h), "/");
    let login_cookie = cookies(&h);
    let (_, _, me) = call(
        &f,
        "GET",
        "/api/web/session",
        &login_cookie,
        "",
        Value::Null,
    )
    .await;
    assert_ne!(me["id"], local.to_string());
    let id = me["id"].as_str().unwrap();
    assert!(
        !f.state
            .control
            .authenticate(
                me["username"].as_str().unwrap(),
                "pab-dummy-credential-not-a-user"
            )
            .await
            .is_ok()
    );
    let pure: bool = sqlx::query_scalar("SELECT password_hash IS NULL FROM users WHERE id=$1")
        .bind(id.parse::<Uuid>().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(pure);
    assert_eq!(
        call(
            &f,
            "DELETE",
            "/api/web/github",
            &login_cookie,
            "",
            Value::Null
        )
        .await
        .2["code"],
        "last_login_method"
    );
    f.profile.lock().await["login"] = json!("renamed-person");
    let (s, c) = begin(&f, "", false).await;
    let (_, h, _) = finish(&f, &s, &c, "good").await;
    assert_eq!(
        call(&f, "GET", "/api/web/session", &cookies(&h), "", Value::Null)
            .await
            .2["id"],
        id
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        call(&f, "GET", "/api/web/github", &login_cookie, "", Value::Null)
            .await
            .2["login"],
        "renamed-person"
    );
    sqlx::query("UPDATE users SET status='disabled' WHERE id=$1")
        .bind(id.parse::<Uuid>().unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let (s, c) = begin(&f, "", false).await;
    let (_, h, _) = finish(&f, &s, &c, "good").await;
    assert!(location(&h).contains("unauthorized"));
}

#[sqlx::test(migrations = "./migrations")]
async fn state_cookie_expiry_replay_cancel_and_upstream_failures(pool: PgPool) {
    let f = fixture(pool.clone(), true).await;
    let (s, c) = begin(&f, "", false).await;
    let (_, h, _) = finish(&f, &s, "", "good").await;
    assert!(location(&h).contains("github_expired"));
    let (_, h, _) = finish(&f, &s, &c, "good").await;
    assert_eq!(location(&h), "/");
    let (_, h, _) = finish(&f, &s, &c, "good").await;
    assert!(location(&h).contains("github_expired"));
    let (s, c) = begin(&f, "", false).await;
    sqlx::query("UPDATE github_authorizations SET expires_at=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(location(&finish(&f, &s, &c, "good").await.1).contains("github_expired"));
    let (s, c) = begin(&f, "", false).await;
    let (_, h, _) = call(
        &f,
        "GET",
        &format!("{CALLBACK}?state={s}&error=access_denied"),
        &c,
        "",
        Value::Null,
    )
    .await;
    assert!(location(&h).contains("github_cancelled"));
    let (s, c) = begin(&f, "", false).await;
    assert!(location(&finish(&f, &s, &c, "invalid").await.1).contains("github_unavailable"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM external_identities")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn binding_preserves_user_rejects_conflicts_and_revoked_sessions(pool: PgPool) {
    let f = fixture(pool.clone(), true).await;
    let (a, token) = password_user(&f, "local-one").await;
    let cookie = session::cookie(&token);
    let (s, c) = begin(&f, &cookie, true).await;
    assert_eq!(
        location(&finish(&f, &s, &c, "good").await.1),
        "/account?github=linked"
    );
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>("SELECT user_id FROM external_identities")
            .fetch_one(&pool)
            .await
            .unwrap(),
        a
    );
    let (_, other) = password_user(&f, "local-two").await;
    let (s, c) = begin(&f, &session::cookie(&other), true).await;
    assert!(location(&finish(&f, &s, &c, "good").await.1).contains("github_already_linked"));
    assert_eq!(
        call(&f, "DELETE", "/api/web/github", &cookie, "", Value::Null)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let (s, c) = begin(&f, &cookie, true).await;
    call(&f, "POST", "/api/web/logout", &cookie, "", Value::Null).await;
    assert!(location(&finish(&f, &s, &c, "good").await.1).contains("github_expired"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM external_identities")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn native_redemption_checks_proof_then_consumes_once(pool: PgPool) {
    let f = fixture(pool.clone(), true).await;
    let verifier = "a".repeat(64);
    let nonce = "b".repeat(64);
    let input = json!({"redirect_uri":"http://127.0.0.1:49001/github/callback","client_state":nonce,"proof_hash":hash(&verifier)});
    let (status, _, value) = call(&f, "POST", "/api/account/github/start", "", "", input).await;
    assert_eq!(status, StatusCode::OK);
    let url = url::Url::parse(value["authorization_url"].as_str().unwrap()).unwrap();
    let path = format!("{}?{}", url.path(), url.query().unwrap());
    let (status, h, _) = call(&f, "GET", &path, "", "", Value::Null).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        call(&f, "GET", &path, "", "", Value::Null).await.0,
        StatusCode::BAD_REQUEST
    );
    let state = query(&location(&h), "state");
    let (_, h, _) = finish(&f, &state, &cookies(&h), "good").await;
    let callback = location(&h);
    assert!(callback.starts_with("http://127.0.0.1:49001/github/callback?"));
    assert_eq!(query(&callback, "state"), nonce);
    let code = query(&callback, "code");
    assert!(!callback.contains("access_token"));
    assert_eq!(
        call(
            &f,
            "POST",
            "/api/account/github/redeem",
            "",
            "",
            json!({"code":code,"verifier":"c".repeat(64)})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status, _, v) = call(
        &f,
        "POST",
        "/api/account/github/redeem",
        "",
        "",
        json!({"code":code,"verifier":verifier}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        call(
            &f,
            "GET",
            "/api/account/session",
            "",
            v["access_token"].as_str().unwrap(),
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &f,
            "POST",
            "/api/account/github/redeem",
            "",
            "",
            json!({"code":code,"verifier":verifier})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn closed_registration_allows_existing_identity_and_concurrent_login_is_unique(pool: PgPool) {
    let mut f = fixture(pool.clone(), true).await;
    let (s1, c1) = begin(&f, "", false).await;
    let (s2, c2) = begin(&f, "", false).await;
    let (a, b) = tokio::join!(finish(&f, &s1, &c1, "good"), finish(&f, &s2, &c2, "good"));
    assert_eq!(location(&a.1), "/");
    assert_eq!(location(&b.1), "/");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    let api = ControlApiState::new(
        (*f.state.control).clone(),
        ControlApiConfig {
            registration_enabled: false,
            ..Default::default()
        },
        RelayControlAuth::new("fixture-relay-control-secret-32-chars").unwrap(),
    );
    let mut api = api;
    api.github = f.state.github.clone();
    f.app = crate::web::router(api);
    let (s, c) = begin(&f, "", false).await;
    assert_eq!(location(&finish(&f, &s, &c, "good").await.1), "/");
    f.profile.lock().await["id"] = json!(9999);
    let (s, c) = begin(&f, "", false).await;
    assert!(location(&finish(&f, &s, &c, "good").await.1).contains("registration_disabled"));
}
