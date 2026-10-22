//! Real authenticated WSS reports and browser reads, using isolated databases.
use axum_server::tls_rustls::RustlsConfig;
use futures_util::{SinkExt, StreamExt};
use pab_protocol::{RelayControlServerMessage, RequestId};
use pab_server::{
    ControlApiConfig, ControlApiState, ControlPlane, PasswordPolicy, PostgresStore,
    RelayControlAuth, control_router,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
    tungstenite::{Message, client::IntoClientRequest},
};

const SECRET: &str = "isolated-telemetry-control-01234567890123456789";
type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
struct Fixture {
    base: String,
    connector: Connector,
    client: reqwest::Client,
    cookie: String,
    handle: axum_server::Handle<std::net::SocketAddr>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(pool: PgPool) -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let control =
            ControlPlane::new(PostgresStore::from_pool(pool), PasswordPolicy::default()).unwrap();
        control.initialize_settings().await.unwrap();
        control
            .register_account("telemetry-admin", "isolated password long enough")
            .await
            .unwrap();
        pab_server::web::bootstrap_admin(&control, "telemetry-admin")
            .await
            .unwrap();
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let tls = RustlsConfig::from_der(
            vec![cert.cert.der().to_vec()],
            cert.signing_key.serialize_der(),
        )
        .await
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!(
            "https://localhost:{}",
            listener.local_addr().unwrap().port()
        );
        let state = ControlApiState::new(
            control,
            ControlApiConfig {
                web_origin: Some(base.clone()),
                ..Default::default()
            },
            RelayControlAuth::new(SECRET).unwrap(),
        );
        let handle = axum_server::Handle::new();
        let shutdown = handle.clone();
        let server = tokio::spawn(async move {
            axum_server::from_tcp_rustls(listener, tls)
                .unwrap()
                .handle(shutdown)
                .serve(control_router(state).into_make_service())
                .await
                .unwrap();
        });
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.cert.der().clone()).unwrap();
        let connector = Connector::Rustls(Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        ));
        let client = reqwest::Client::builder()
            .no_proxy()
            .add_root_certificate(
                reqwest::Certificate::from_pem(cert.cert.pem().as_bytes()).unwrap(),
            )
            .build()
            .unwrap();
        let login = client
            .post(format!("{base}/api/web/session"))
            .header("Origin", &base)
            .json(&json!({"username":"telemetry-admin","password":"isolated password long enough"}))
            .send()
            .await
            .unwrap();
        assert!(login.status().is_success());
        let cookie = login.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        Self {
            base,
            connector,
            client,
            cookie,
            handle,
            server,
        }
    }
    async fn socket(&self) -> Socket {
        let mut request = format!("{}/relay-control", self.base.replace("https://", "wss://"))
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {SECRET}").parse().unwrap());
        connect_async_tls_with_config(request, None, false, Some(self.connector.clone()))
            .await
            .unwrap()
            .0
    }
    async fn node(&self, id: &str) -> Value {
        let response = self
            .client
            .get(format!("{}/api/web/relays", self.base))
            .header("cookie", &self.cookie)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let value: Value = response.json().await.unwrap();
        assert_eq!(value["mode"], "legacy");
        value["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == id)
            .unwrap()
            .clone()
    }
    async fn shutdown(self) {
        self.handle
            .graceful_shutdown(Some(Duration::from_millis(100)));
        self.server.await.unwrap();
    }
}
async fn report(
    socket: &mut Socket,
    node: &str,
    telemetry: Option<Value>,
) -> RelayControlServerMessage {
    let mut value = json!({"type":"get_policy","request_id":RequestId::new(),"known_policy_version":null,"node_id":node,"agent_version":"telemetry-test"});
    if let Some(sample) = telemetry {
        value["telemetry"] = sample;
    }
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await.unwrap().unwrap() {
                Message::Text(text) => {
                    let response = serde_json::from_str(&text).unwrap();
                    if !matches!(response, RelayControlServerMessage::PolicyChanged) {
                        break response;
                    }
                }
                Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await.unwrap(),
                _ => {}
            }
        }
    })
    .await
    .unwrap()
}
fn sample(boot: uuid::Uuid, connections: u64, received: u64, sent: u64) -> Value {
    json!({"boot_id":boot,"public_url":"https://relay.example/","connections":connections,"received_bytes":received,"sent_bytes":sent})
}
fn accepted(response: RelayControlServerMessage) {
    assert!(matches!(
        response,
        RelayControlServerMessage::PolicySnapshot { .. }
            | RelayControlServerMessage::PolicyUnchanged { .. }
    ));
}

fn diagnostics() -> Value {
    json!({"version":1,"host":{"hostname":"isolated-relay","os":"linux","os_version":"Linux fixture","architecture":"x86_64","cpu_model":"Fixture CPU","logical_cpus":4,"cpu_usage_basis_points":1250,"memory_total":8589934592u64,"memory_used":1073741824u64,"host_uptime_ms":86400000,"process_uptime_ms":1000,"process_memory":33554432,"containerized":true},"traffic":{"accepted_connections":2,"disconnected_connections":0,"received_bytes":9007199254740993u64,"sent_bytes":2048,"received_packets":3,"sent_packets":3,"dropped_packets":0,"rate_limited_bytes":0,"ping_frames":1,"received_bytes_per_second":1234,"sent_bytes_per_second":5678},"connections_total":2,"links_total":1,"active_links":1,"connections_truncated":false,"links_truncated":false,"connections":[{"id":"1","endpoint":vec![5u8;32],"probe":false,"age_ms":1000,"connected_ms":1000,"closed_ago_ms":null},{"id":"2","endpoint":vec![6u8;32],"probe":false,"age_ms":1000,"connected_ms":1000,"closed_ago_ms":null}],"links":[{"id":uuid::Uuid::new_v4(),"a":vec![5u8;32],"b":vec![6u8;32],"age_ms":1000,"observed_ms":1000,"last_forwarded_ago_ms":0,"closed_ago_ms":null,"a_to_b_bytes":9007199254740993u64,"b_to_a_bytes":2048,"a_to_b_packets":1,"b_to_a_packets":2,"a_to_b_bytes_per_second":1234,"b_to_a_bytes_per_second":5678}]})
}

#[sqlx::test(migrations = "./migrations")]
async fn diagnostic_inventory_maps_devices_filters_paginates_and_fences_stale_processes(pool:PgPool) {
    let fixture=Fixture::new(pool.clone()).await;
    let user:uuid::Uuid=sqlx::query_scalar("SELECT id FROM users WHERE username='telemetry-admin'").fetch_one(&pool).await.unwrap();
    let tenant:uuid::Uuid=sqlx::query_scalar("SELECT tenant_id FROM personal_tenants WHERE user_id=$1").bind(user).fetch_one(&pool).await.unwrap();
    let device=uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO devices(id,tenant_id,name,code) VALUES($1,$2,'Android phone',123456789)").bind(device).bind(tenant).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO device_accounts(device_id,user_id) VALUES($1,$2)").bind(device).bind(user).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO endpoints(endpoint_key,tenant_id,owner_kind,device_id) VALUES($1,$2,'device',$3)").bind(vec![5u8;32]).bind(tenant).bind(device).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO endpoints(endpoint_key,tenant_id,owner_kind,user_id) VALUES($1,$2,'user',$3)").bind(vec![6u8;32]).bind(tenant).bind(user).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO device_runtime(tenant_id,device_id,execution_context,environment_revision,agent_version,observed_at_unix_ms) VALUES($1,$2,'{\"os_family\":\"android\",\"architecture\":\"aarch64\"}'::jsonb,'fixture','1.3.19',1000)").bind(tenant).bind(device).execute(&pool).await.unwrap();
    let mut socket=fixture.socket().await;let boot=uuid::Uuid::new_v4();let mut telemetry=sample(boot,2,100,200);telemetry["diagnostics"]=diagnostics();
    accepted(report(&mut socket,"primary",Some(telemetry.clone())).await);
    let get=|query:&str| fixture.client.get(format!("{}/api/web/relays/primary/runtime?{query}",fixture.base)).header("cookie",&fixture.cookie);
    let data:Value=get("view=connections").send().await.unwrap().json().await.unwrap();
    assert_eq!(data["fresh"],true);assert_eq!(data["summary"]["connections"],"2");assert_eq!(data["summary"]["active_links"],1);assert_eq!(data["traffic"]["received_bytes"],"9007199254740993");
    assert_eq!(data["items"][0]["identity"]["name"],"Android phone");assert_eq!(data["items"][0]["identity"]["system"],"android");assert_eq!(data["items"][0]["identity"]["username"],"telemetry-admin");
    let links:Value=get("view=links&status=active").send().await.unwrap().json().await.unwrap();assert_eq!(links["total"],1);assert_eq!(links["items"][0]["a_to_b_bytes"],"9007199254740993");
    let filtered:Value=get("view=connections&q=Android").send().await.unwrap().json().await.unwrap();assert_eq!(filtered["total"],1);assert_eq!(filtered["summary"]["connections"],"2");
    let empty:Value=get("q=architecture").send().await.unwrap().json().await.unwrap();assert_eq!(empty["total"],0);
    let page:Value=get("page_size=1&page=2").send().await.unwrap().json().await.unwrap();assert_eq!(page["total"],2);assert_eq!(page["items"][0]["identity"]["kind"],"user");
    assert_eq!(get("view=connections&status=active").send().await.unwrap().status(),400);
    assert_eq!(fixture.client.get(format!("{}/api/web/relays/primary/runtime",fixture.base)).send().await.unwrap().status(),401);
    // A late same-process snapshot cannot move counters or its observation clock backwards.
    telemetry["diagnostics"]["host"]["process_uptime_ms"]=json!(1);telemetry["diagnostics"]["host"]["hostname"]=json!("late");accepted(report(&mut socket,"primary",Some(telemetry.clone())).await);
    let unchanged:Value=get("").send().await.unwrap().json().await.unwrap();assert_eq!(unchanged["host"]["hostname"],"isolated-relay");
    // A larger, valid report crosses the old 64 KiB frame limit without dropping the control socket.
    let base=diagnostics();let mut large=base.clone();large["host"]["process_uptime_ms"]=json!(2000);large["connections_total"]=json!(1024);large["connections"]=Value::Array((0..1024).map(|id|{let mut row=base["connections"][0].clone();row["id"]=json!(id.to_string());row}).collect());telemetry["diagnostics"]=large;
    accepted(report(&mut socket,"primary",Some(telemetry)).await);
    let large:Value=get("page_size=100&page=11").send().await.unwrap().json().await.unwrap();assert_eq!(large["total"],1024);assert_eq!(large["items"].as_array().unwrap().len(),24);
    // A new node boot makes the retained old sample historical, never a live count.
    sqlx::query("UPDATE relay_nodes SET telemetry_boot_id=$1 WHERE node_id='primary'").bind(uuid::Uuid::new_v4()).execute(&pool).await.unwrap();
    let stale:Value=get("").send().await.unwrap().json().await.unwrap();assert_eq!(stale["fresh"],false);assert!(stale["summary"]["connections"].is_null());assert_eq!(stale["items"][0]["status"],"unknown");
    socket.close(None).await.unwrap();fixture.shutdown().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn reports_exact_counters_reset_after_restart_and_fence_late_connections(pool: PgPool) {
    let fixture = Fixture::new(pool.clone()).await;
    let mut old = fixture.socket().await;
    let boot = uuid::Uuid::new_v4();
    accepted(
        report(
            &mut old,
            "primary",
            Some(sample(boot, 7, u64::MAX, 9_007_199_254_740_993)),
        )
        .await,
    );
    let node = fixture.node("primary").await;
    assert_eq!(node["url"], "https://relay.example/");
    assert_eq!(node["capacity_mode"], "host");
    assert_eq!(node["connections_exact"], "7");
    assert_eq!(node["uploaded_bytes_exact"], u64::MAX.to_string());
    assert_eq!(node["downloaded_bytes_exact"], "9007199254740993");
    assert_eq!(node["telemetry_current"], true);
    let mut new = fixture.socket().await;
    let new_boot = uuid::Uuid::new_v4();
    accepted(report(&mut new, "primary", Some(sample(new_boot, 0, 0, 0))).await);
    accepted(
        report(
            &mut old,
            "primary",
            Some(sample(boot, 99, u64::MAX, u64::MAX)),
        )
        .await,
    );
    let node = fixture.node("primary").await;
    assert_eq!(node["boot_id"], new_boot.to_string());
    assert_eq!(node["connections_exact"], "0");
    assert_eq!(node["uploaded_bytes_exact"], "0");
    accepted(report(&mut new, "primary", Some(sample(new_boot, 2, 1024, 2048))).await);
    assert_eq!(
        fixture.node("primary").await["downloaded_bytes_exact"],
        "2048"
    );
    // A policy heartbeat alone cannot refresh stale process data.
    sqlx::query(
        "UPDATE relay_nodes SET telemetry_received_at=clock_timestamp()-interval '121 seconds'",
    )
    .execute(&pool)
    .await
    .unwrap();
    accepted(report(&mut new, "primary", None).await);
    let stale = fixture.node("primary").await;
    assert_eq!(stale["online"], true);
    assert_eq!(stale["telemetry_current"], false);
    assert!(stale["connections_exact"].is_null() && stale["uploaded_bytes_exact"].is_null());
    // Samples from the previous Server process must also remain unknown.
    sqlx::query("UPDATE relay_nodes SET telemetry_server_instance=$1,telemetry_received_at=clock_timestamp()").bind(uuid::Uuid::new_v4()).execute(&pool).await.unwrap();
    assert_eq!(fixture.node("primary").await["telemetry_current"], false);
    accepted(report(&mut new, "primary", Some(sample(new_boot, 1, 2048, 4096))).await);
    assert_eq!(fixture.node("primary").await["telemetry_current"], true);
    old.close(None).await.unwrap();
    new.close(None).await.unwrap();
    fixture.shutdown().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn old_peers_keep_policy_access_and_invalid_reports_do_not_create_metadata(pool: PgPool) {
    let fixture = Fixture::new(pool.clone()).await;
    let mut old = fixture.socket().await;
    accepted(report(&mut old, "old-relay", None).await);
    let old_node = fixture.node("old-relay").await;
    assert!(old_node["connections_exact"].is_null() && old_node["capacity_mode"].is_null());
    let boot = uuid::Uuid::new_v4();
    for url in [
        "http://relay.example/",
        "https://user:private@relay.example/",
        "https://relay.example/?token=private",
        "https://relay.example/#private",
    ] {
        let mut bad = sample(boot, 0, 0, 0);
        bad["public_url"] = json!(url);
        let response = report(&mut old, "invalid-relay", Some(bad)).await;
        assert!(matches!(response, RelayControlServerMessage::Error { .. }));
        assert!(!format!("{response:?}").contains("private"));
    }
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM relay_nodes WHERE node_id='invalid-relay'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    accepted(report(&mut old, "old-relay", Some(sample(boot, 0, 0, 0))).await);
    assert!(matches!(
        report(
            &mut old,
            "old-relay",
            Some(sample(uuid::Uuid::new_v4(), 0, 0, 0))
        )
        .await,
        RelayControlServerMessage::Error { .. }
    ));
    old.close(None).await.unwrap();
    fixture.shutdown().await;
}
