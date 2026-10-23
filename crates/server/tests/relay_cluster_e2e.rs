//! Real WSS control + policy-enforcing TLS Relays + Relay-only iroh endpoints.
use axum_server::tls_rustls::RustlsConfig;
use iroh_base::SecretKey;
use pab_agent_core::{
    EndpointControlConfig, EndpointControlSupervisor, ReconnectPolicy, tls_connector,
};
use pab_protocol::*;
use pab_relay::{RelayServiceConfig, start_relay_service};
use pab_server::{
    ControlApiConfig, ControlApiState, ControlPlane, PasswordPolicy, PostgresStore,
    RelayControlAuth, control_router,
    relay_cluster::{RelayClusterConfig, RelayNodeConfig},
};
use pab_transport::{ConnectionPath, PabEndpoint, PabEndpointAddress, PabEndpointConfig};
use sqlx::PgPool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
fn reserve() -> std::net::TcpListener {
    std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap()
}
async fn wait_home(endpoint: &PabEndpoint, url: &str) {
    let mut addresses = endpoint.watch_address();
    tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            if addresses.borrow().relay_urls == [url.to_string()] {
                break;
            }
            addresses.changed().await.unwrap();
        }
    })
    .await
    .expect("bounded directory propagation/home switch");
}
async fn echo(a: &PabEndpoint, b: &PabEndpoint) {
    let address = PabEndpointAddress {
        relay_urls: b.address().relay_urls().map(ToString::to_string).collect(),
        direct_addresses: vec![],
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        let (client, server) = tokio::join!(
            a.connect(*b.id().as_bytes(), &address, Duration::from_secs(25)),
            b.accept()
        );
        let client = client.unwrap();
        let server = server.unwrap().unwrap();
        assert_eq!(client.selected_path(), Some(ConnectionPath::Relay));
        let expected = vec![179u8; 12_000];
        let send = async {
            let mut stream = client.open_bi(Duration::from_secs(5)).await.unwrap();
            stream
                .send_json(&expected, Duration::from_secs(5))
                .await
                .unwrap();
            let actual: Vec<u8> = stream.receive_json(Duration::from_secs(5)).await.unwrap();
            assert_eq!(actual, expected);
        };
        let receive = async {
            let mut stream = server.accept_bi(Duration::from_secs(5)).await.unwrap();
            let bytes: Vec<u8> = stream.receive_json(Duration::from_secs(5)).await.unwrap();
            assert_eq!(bytes, expected);
            stream
                .send_json(&bytes, Duration::from_secs(5))
                .await
                .unwrap();
        };
        tokio::join!(send, receive);
        assert_eq!(client.direct_traffic().uploaded_bytes, 0);
        assert_eq!(client.direct_traffic().downloaded_bytes, 0);
        client.close(b"probe complete");
        server.close(b"probe complete");
    })
    .await
    .expect("authorized Relay echo after failover");
}

/// Bounded memory, large enough to cross multiple 15-second budget renewals.
/// A returned hash is also an application-level receipt, not merely a successful write.
async fn large_transfer(a: &PabEndpoint, b: &PabEndpoint, count: usize) {
    const CHUNK: usize = 64 * 1024;
    let address = PabEndpointAddress {
        relay_urls: b.address().relay_urls().map(ToString::to_string).collect(),
        direct_addresses: vec![],
    };
    tokio::time::timeout(Duration::from_secs(150), async {
        let (client, server) = tokio::join!(
            a.connect(*b.id().as_bytes(), &address, Duration::from_secs(25)),
            b.accept()
        );
        let client = client.unwrap();
        let server = server.unwrap().unwrap();
        assert_eq!(client.selected_path(), Some(ConnectionPath::Relay));
        let started = std::time::Instant::now();
        let send = async {
            let mut stream = client.open_bi(Duration::from_secs(10)).await.unwrap();
            let mut expected = blake3::Hasher::new();
            for index in 0..count {
                let mut bytes = vec![0u8; CHUNK];
                for (offset, byte) in bytes.iter_mut().enumerate() {
                    *byte = (offset as u8).wrapping_mul(71) ^ (index as u8);
                }
                expected.update(&bytes);
                stream
                    .send_binary_frame(&bytes, Duration::from_secs(20))
                    .await
                    .unwrap();
            }
            let receipt: Vec<u8> = stream.receive_json(Duration::from_secs(20)).await.unwrap();
            assert_eq!(receipt, expected.finalize().as_bytes());
        };
        let receive = async {
            let mut stream = server.accept_bi(Duration::from_secs(10)).await.unwrap();
            let mut actual = blake3::Hasher::new();
            for _ in 0..count {
                let bytes = stream
                    .receive_binary_frame(Duration::from_secs(20))
                    .await
                    .unwrap();
                assert_eq!(bytes.len(), CHUNK);
                actual.update(&bytes);
            }
            stream
                .send_json(
                    &actual.finalize().as_bytes().to_vec(),
                    Duration::from_secs(10),
                )
                .await
                .unwrap();
        };
        tokio::join!(send, receive);
        // Default account is 10 Mbps shared by all its endpoints/nodes. Even
        // allowing the 100-ms initial burst, 64 MiB must not complete too early.
        if count >= 1024 {
            assert!(
                started.elapsed() >= Duration::from_secs(50),
                "aggregate account budget was exceeded"
            );
        }
        eprintln!(
            "Relay-only transfer, bytes={}, hash matched, elapsed_ms={}",
            count * CHUNK,
            started.elapsed().as_millis()
        );
        client.close(b"large transfer complete");
        server.close(b"large transfer complete");
    })
    .await
    .expect("64 MiB forwarding must make progress across budget renewals");
}

#[sqlx::test(migrations = "./migrations")]
async fn directory_and_budget_control_drive_real_relay_failover(pool: PgPool) {
    cluster_failover(pool, false, false, false).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn three_nodes_forward_and_fail_over_twice_without_overlapping_maintenance(pool: PgPool) {
    cluster_failover(pool, true, false, false).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn diagnostics_follow_real_relay_only_forwarding_and_disconnects(pool:PgPool) {
    cluster_failover(pool,false,true,false).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn disable_closes_real_sockets_denies_reconnect_and_preserves_p2p(pool: PgPool) {
    cluster_failover(pool,false,false,true).await;
}

async fn wait_operation(store:&PostgresStore, id:uuid::Uuid, phase:&str) {
    tokio::time::timeout(Duration::from_secs(20),async {loop {
        let operation=store.relay_maintenance_operation(id).await.unwrap();
        if operation.phase==phase {break;}
        assert_ne!(operation.phase,"failed","node operation failed: {:?}",operation);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }}).await.expect("node confirmation must arrive");
}
async fn set_web_state(client:&reqwest::Client,origin:&str,cookie:&str,node:&str,state:&str)->uuid::Uuid {
    let id=uuid::Uuid::new_v4();let result=client.patch(format!("{origin}/api/web/relays/{node}/state")).header("Origin",origin).header("cookie",cookie).json(&serde_json::json!({"state":state,"operation_id":id})).send().await.unwrap();
    assert!(result.status().is_success(),"state request: {:?}",result.text().await.unwrap());id
}
async fn direct_echo(client:&pab_transport::PabConnection,server:&pab_transport::PabConnection) {
    assert_eq!(client.selected_path(),Some(ConnectionPath::Direct));
    let send=async {let mut stream=client.open_bi(Duration::from_secs(5)).await.unwrap();stream.send_json(&"direct still works",Duration::from_secs(5)).await.unwrap();let answer:String=stream.receive_json(Duration::from_secs(5)).await.unwrap();assert_eq!(answer,"direct still works");};
    let receive=async {let mut stream=server.accept_bi(Duration::from_secs(5)).await.unwrap();let data:String=stream.receive_json(Duration::from_secs(5)).await.unwrap();stream.send_json(&data,Duration::from_secs(5)).await.unwrap();};
    tokio::join!(send,receive);
}

async fn cluster_failover(pool: PgPool, three_nodes: bool, diagnostics_only:bool, node_control_only:bool) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let store = PostgresStore::from_pool(pool.clone());
    store.initialize_settings().await.unwrap();
    let base = ControlPlane::new(store.clone(), PasswordPolicy::default()).unwrap();
    let account = base
        .register_account("cluster-e2e", "isolated test password long enough")
        .await
        .unwrap();
    if diagnostics_only || node_control_only {pab_server::web::bootstrap_admin(&base,"cluster-e2e").await.unwrap();}
    let operator = SecretKey::from_bytes(&[211; 32]);
    let device = SecretKey::from_bytes(&[212; 32]);
    let device_id = DeviceId::new();
    let device_ref = DeviceRef {
        tenant_id: account.personal_tenant_id,
        device_id,
    };
    sqlx::query(
        "INSERT INTO devices(id,tenant_id,name,code) VALUES($1,$2,'cluster fixture',619438177)",
    )
    .bind(device_id.as_uuid())
    .bind(account.personal_tenant_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO endpoints(endpoint_key,tenant_id,owner_kind,user_id) VALUES($1,$2,'user',$3)",
    )
    .bind(operator.public().as_bytes().as_slice())
    .bind(account.personal_tenant_id.as_uuid())
    .bind(account.id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO endpoints(endpoint_key,tenant_id,owner_kind,device_id) VALUES($1,$2,'device',$3)")
        .bind(device.public().as_bytes().as_slice()).bind(account.personal_tenant_id.as_uuid()).bind(device_id.as_uuid()).execute(&pool).await.unwrap();
    let hash = "b".repeat(64);
    sqlx::query("INSERT INTO web_sessions(token_hash,user_id) VALUES($1,$2)")
        .bind(&hash)
        .bind(account.id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO endpoint_user_contexts(endpoint_key,token_hash,revision) VALUES($1,$2,1)",
    )
    .bind(operator.public().as_bytes().as_slice())
    .bind(&hash)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO device_connection_intents(operator_endpoint_key,device_id,expires_at) VALUES($1,$2,clock_timestamp()+interval '5 minutes')")
        .bind(operator.public().as_bytes().as_slice()).bind(device_id.as_uuid()).execute(&pool).await.unwrap();
    let reserved_a = reserve();
    let reserved_b = reserve();
    let reserved_c = three_nodes.then(reserve);
    let port_a = reserved_a.local_addr().unwrap().port();
    let port_b = reserved_b.local_addr().unwrap().port();
    let mut config = RelayClusterConfig {
        heartbeat_seconds: 1,
        offline_seconds: 3,
        recovery_seconds: 0,
        maintenance_seconds: if node_control_only {2} else {0},
        nodes: vec![
            RelayNodeConfig {
                node_id: "a".into(),
                url: format!("https://localhost:{port_a}/"),
                region: "test".into(),
                weight: 100,
                capacity_mbps: if three_nodes { 100 } else { 0 },
                control_secret: "isolated-cluster-node-a-01234567890123456789".into(),
            },
            RelayNodeConfig {
                node_id: "b".into(),
                url: format!("https://localhost:{port_b}/"),
                region: "test".into(),
                weight: 100,
                capacity_mbps: if three_nodes { 100 } else { 0 },
                control_secret: "isolated-cluster-node-b-01234567890123456789".into(),
            },
        ],
    };
    if let Some(listener) = &reserved_c {
        config.nodes.push(RelayNodeConfig {
            node_id: "c".into(),
            url: format!(
                "https://localhost:{}/",
                listener.local_addr().unwrap().port()
            ),
            region: "test".into(),
            weight: 100,
            capacity_mbps: if three_nodes { 100 } else { 0 },
            control_secret: "isolated-cluster-node-c-01234567890123456789".into(),
        });
    }
    let control = base.with_relay_cluster(Some(config.clone())).await.unwrap();
    // Start B in maintenance so initial traffic is provably carried by A.
    sqlx::query(
        "UPDATE relay_cluster_nodes SET admin_state='maintenance' WHERE node_id IN ('b','c')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let auth = RelayControlAuth::new("isolated-legacy-control-01234567890123456789")
        .unwrap()
        .with_cluster(Some(&config));
    let state = ControlApiState::new(control, ControlApiConfig::default(), auth);
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let root = tempfile::tempdir().unwrap();
    let cert = root.path().join("cert.pem");
    let key = root.path().join("key.pem");
    std::fs::write(&cert, certified.cert.pem()).unwrap();
    std::fs::write(&key, certified.signing_key.serialize_pem()).unwrap();
    let tls = RustlsConfig::from_der(
        vec![certified.cert.der().to_vec()],
        certified.signing_key.serialize_der(),
    )
    .await
    .unwrap();
    let listener = reserve();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = axum_server::Handle::new();
    let server_handle = handle.clone();
    let server = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .unwrap()
            .handle(server_handle)
            .serve(control_router(state).into_make_service())
            .await
            .unwrap()
    });
    let relay_config = |index: usize| RelayServiceConfig {
        cluster_enabled: true,
        public_url: None,
        node_id: config.nodes[index].node_id.clone(),
        usage_dir: root.path().join(format!("usage-{index}")),
        control_url: format!("wss://localhost:{port}/relay-control"),
        control_secret: config.nodes[index].control_secret.clone(),
        control_ca_cert: Some(cert.clone()),
        tls_cert: cert.clone(),
        tls_key: key.clone(),
        https_bind: format!(
            "127.0.0.1:{}",
            url::Url::parse(&config.nodes[index].url)
                .unwrap()
                .port()
                .unwrap()
        )
        .parse()
        .unwrap(),
        captive_bind: "127.0.0.1:0".parse().unwrap(),
        quic_bind: "127.0.0.1:0".parse().unwrap(),
        policy_refresh_interval: Duration::from_secs(1),
        reconnect_interval: Duration::from_millis(100),
        limiter_burst: Duration::from_millis(100),
    };
    drop(reserved_a);
    // A restored database can retain a fresh heartbeat from a crashed process.
    // Startup must respect that lease and retry without admitting both boots.
    let startup = std::time::Instant::now();
    if diagnostics_only {
        store
            .register_relay_boot("a", uuid::Uuid::new_v4(), "crashed-test-process", true, &config, uuid::Uuid::new_v4())
            .await
            .unwrap();
    }
    let relay_a = start_relay_service(relay_config(0)).await.unwrap();
    if diagnostics_only {
        assert!(startup.elapsed() >= Duration::from_secs(2));
    }
    drop(reserved_b);
    let mut relay_b = Some(start_relay_service(relay_config(1)).await.unwrap());
    let relay_c = if three_nodes {
        drop(reserved_c);
        Some(start_relay_service(relay_config(2)).await.unwrap())
    } else {
        None
    };
    let seed: iroh_base::RelayUrl = config.nodes[0].url.parse().unwrap();
    let ep_config = || {
        PabEndpointConfig::new(vec![seed.clone()])
            .unwrap()
            .with_extra_ca_certificates(vec![certified.cert.der().clone()])
            .relay_only_for_testing()
    };
    let a = PabEndpoint::bind(ep_config(), operator.clone())
        .await
        .unwrap();
    let b = PabEndpoint::bind(ep_config(), device.clone())
        .await
        .unwrap();
    let connector = tls_connector(Some(certified.cert.pem().as_bytes())).unwrap();
    let supervisor = |secret: SecretKey, principal| {
        EndpointControlSupervisor::new(
            EndpointControlConfig {
                url: format!("wss://localhost:{port}/control"),
                tenant_id: account.personal_tenant_id,
                principal,
                operation_timeout: Duration::from_secs(10),
            },
            secret,
            connector.clone(),
            ReconnectPolicy {
                heartbeat_interval: Duration::from_secs(1),
                heartbeat_timeout: Duration::from_millis(800),
                retry_interval: Duration::from_millis(100),
            },
        )
        .unwrap()
        .with_relay_directory()
    };
    let operator_control = supervisor(
        operator,
        EndpointProofPrincipal::User {
            user_id: account.id,
        },
    )
    .spawn();
    a.follow_relay_updates(operator_control.relay_updates());
    let mut addresses = b.watch_address();
    let device_key = EndpointKey::new(*device.public().as_bytes());
    let network = move |address: pab_transport::PabEndpointAddress, revision| DeviceNetworkUpdate {
        schema_version: DEVICE_NETWORK_SCHEMA_VERSION,
        device_ref,
        endpoint_key: device_key,
        endpoint_instance_id: EndpointInstanceId::from_u128(221),
        address_revision: revision,
        relay_urls: address.relay_urls,
        direct_addresses: vec![],
        observed_at_unix_ms: now(),
    };
    let (network_tx, network_rx) =
        tokio::sync::watch::channel(network(addresses.borrow().clone(), 1));
    let network_task = tokio::spawn(async move {
        let mut revision = 1;
        while addresses.changed().await.is_ok() {
            revision += 1;
            network_tx.send_replace(network(addresses.borrow_and_update().clone(), revision));
        }
    });
    let device_control = supervisor(device.clone(), EndpointProofPrincipal::Device { device_id })
        .with_device_network(network_rx)
        .unwrap()
        .spawn();
    b.follow_relay_updates(device_control.relay_updates());
    wait_home(&a, &config.nodes[0].url).await;
    wait_home(&b, &config.nodes[0].url).await;
    echo(&a, &b).await;
    if node_control_only {
        let client=reqwest::Client::builder().no_proxy().add_root_certificate(reqwest::Certificate::from_pem(certified.cert.pem().as_bytes()).unwrap()).build().unwrap();
        let origin=format!("https://localhost:{port}");
        let login=client.post(format!("{origin}/api/web/session")).header("Origin",&origin).json(&serde_json::json!({"username":"cluster-e2e","password":"isolated test password long enough"})).send().await.unwrap();assert!(login.status().is_success());
        let cookie=login.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_owned();
        let resumed=set_web_state(&client,&origin,&cookie,"b","active").await;wait_operation(&store,resumed,"completed").await;
        // An independent real P2P connection keeps the original node as its seed.
        let direct_operator=SecretKey::from_bytes(&[216;32]);let direct_device=SecretKey::from_bytes(&[217;32]);let direct_device_id=DeviceId::new();
        sqlx::query("INSERT INTO devices(id,tenant_id,name,code) VALUES($1,$2,'direct node control fixture',619438179)").bind(direct_device_id.as_uuid()).bind(account.personal_tenant_id.as_uuid()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO endpoints(endpoint_key,tenant_id,owner_kind,user_id) VALUES($1,$2,'user',$3)").bind(direct_operator.public().as_bytes().as_slice()).bind(account.personal_tenant_id.as_uuid()).bind(account.id.as_uuid()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO endpoints(endpoint_key,tenant_id,owner_kind,device_id) VALUES($1,$2,'device',$3)").bind(direct_device.public().as_bytes().as_slice()).bind(account.personal_tenant_id.as_uuid()).bind(direct_device_id.as_uuid()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO endpoint_user_contexts(endpoint_key,token_hash,revision) VALUES($1,$2,1)").bind(direct_operator.public().as_bytes().as_slice()).bind(&hash).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO device_connection_intents(operator_endpoint_key,device_id,expires_at) VALUES($1,$2,clock_timestamp()+interval '5 minutes')").bind(direct_operator.public().as_bytes().as_slice()).bind(direct_device_id.as_uuid()).execute(&pool).await.unwrap();
        sqlx::query("UPDATE server_settings SET policy_revision=policy_revision+1").execute(&pool).await.unwrap();
        let normal=|| PabEndpointConfig::new(vec![seed.clone()]).unwrap().with_extra_ca_certificates(vec![certified.cert.der().clone()]);
        let direct_a=PabEndpoint::bind(normal(),direct_operator).await.unwrap();let direct_b=PabEndpoint::bind(normal(),direct_device).await.unwrap();
        direct_a.wait_online(Duration::from_secs(10)).await.unwrap();direct_b.wait_online(Duration::from_secs(10)).await.unwrap();
        let direct_address=PabEndpointAddress{relay_urls:vec![],direct_addresses:direct_b.address().ip_addrs().copied().collect()};assert!(!direct_address.direct_addresses.is_empty());
        let (held_client,held_server)=tokio::join!(direct_a.connect(*direct_b.id().as_bytes(),&direct_address,Duration::from_secs(10)),direct_b.accept());let held_client=held_client.unwrap();let held_server=held_server.unwrap().unwrap();direct_echo(&held_client,&held_server).await;
        let disabled=set_web_state(&client,&origin,&cookie,"a","disabled").await;wait_operation(&store,disabled,"completed").await;
        let count:i64=sqlx::query_scalar("SELECT connections FROM relay_cluster_nodes WHERE node_id='a'").fetch_one(&pool).await.unwrap();assert_eq!(count,0,"all accepted Relay sockets must actually close");
        let denied=PabEndpoint::bind(ep_config(),device.clone()).await.unwrap();assert!(denied.wait_online(Duration::from_secs(3)).await.is_err(),"a known identity cannot reconnect to a disabled node");denied.close().await;
        direct_echo(&held_client,&held_server).await;
        wait_home(&a,&config.nodes[1].url).await;wait_home(&b,&config.nodes[1].url).await;echo(&a,&b).await;
        let resumed=set_web_state(&client,&origin,&cookie,"a","active").await;wait_operation(&store,resumed,"completed").await;
        let accepted=PabEndpoint::bind(ep_config(),device.clone()).await.unwrap();accepted.wait_online(Duration::from_secs(10)).await.unwrap();
        let maintenance=set_web_state(&client,&origin,&cookie,"a","maintenance").await;wait_operation(&store,maintenance,"ready").await;
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT connections FROM relay_cluster_nodes WHERE node_id='a'").fetch_one(&pool).await.unwrap(),0);
        let denied=PabEndpoint::bind(ep_config(),device.clone()).await.unwrap();assert!(denied.wait_online(Duration::from_secs(3)).await.is_err());denied.close().await;
        direct_echo(&held_client,&held_server).await;
        accepted.close().await;held_client.close(b"node control test complete");held_server.close(b"node control test complete");direct_a.close().await;direct_b.close().await;
        a.close().await;b.close().await;operator_control.shutdown().await.unwrap();device_control.shutdown().await.unwrap();network_task.abort();relay_a.shutdown().await.unwrap();relay_b.take().unwrap().shutdown().await.unwrap();handle.shutdown();server.await.unwrap();return;
    }
    if diagnostics_only {
        let client=reqwest::Client::builder().no_proxy().add_root_certificate(reqwest::Certificate::from_pem(certified.cert.pem().as_bytes()).unwrap()).build().unwrap();
        let origin=format!("https://localhost:{port}");
        let login=client.post(format!("{origin}/api/web/session")).header("Origin",&origin).json(&serde_json::json!({"username":"cluster-e2e","password":"isolated test password long enough"})).send().await.unwrap();assert!(login.status().is_success());
        let cookie=login.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_owned();
        let endpoint=format!("{origin}/api/web/relays/a/runtime?view=links");
        let sample=tokio::time::timeout(Duration::from_secs(12),async {loop{
            let sample:serde_json::Value=client.get(&endpoint).header("cookie",&cookie).send().await.unwrap().json().await.unwrap();
            if sample["fresh"]==true&&sample["summary"]["active_links"].as_u64().unwrap_or(0)>0 {break sample;}
            tokio::time::sleep(Duration::from_millis(100)).await;
        }}).await.expect("real forwarding inventory must be reported");
        assert_eq!(sample["summary"]["connected_endpoints"],2);
        let link=&sample["items"][0];assert_eq!(link["status"],"active");
        let bytes=link["a_to_b_bytes"].as_str().unwrap().parse::<u64>().unwrap()+link["b_to_a_bytes"].as_str().unwrap().parse::<u64>().unwrap();assert!(bytes>12_000);
        assert!(link["a"]["name"]=="cluster fixture"||link["b"]["name"]=="cluster fixture");
        assert!(link["a"]["username"]=="cluster-e2e"||link["b"]["username"]=="cluster-e2e");
        assert!(sample["host"]["logical_cpus"].as_u64().unwrap()>0);
        a.close().await;
        tokio::time::timeout(Duration::from_secs(10),async {loop{
            let sample:serde_json::Value=client.get(&endpoint).header("cookie",&cookie).send().await.unwrap().json().await.unwrap();
            if sample["items"].as_array().is_some_and(|rows|rows.iter().any(|row|row["status"]=="closed")){break;}
            tokio::time::sleep(Duration::from_millis(100)).await;
        }}).await.expect("disconnects must close observed pairs");
        b.close().await;operator_control.shutdown().await.unwrap();device_control.shutdown().await.unwrap();network_task.abort();
        relay_a.shutdown().await.unwrap();relay_b.take().unwrap().shutdown().await.unwrap();handle.shutdown();server.await.unwrap();return;
    }
    large_transfer(&a, &b, 1024).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let directory = store.relay_directory(3).await.unwrap();
            if directory
                .nodes
                .iter()
                .all(|n| n.health == RelayHealth::Healthy)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    store
        .set_relay_state(
            "b",
            RelayAdminState::Active,
            uuid::Uuid::new_v4(),
            account.id.as_uuid(),
            false,
            0,
            3,
        )
        .await
        .unwrap();
    // Two independent endpoint pairs under the same account generate bulk
    // traffic on both nodes simultaneously; single-node timing alone cannot
    // prove the aggregate cluster limit.
    let second_operator = SecretKey::from_bytes(&[213; 32]);
    let second_device = SecretKey::from_bytes(&[214; 32]);
    let second_device_id = DeviceId::new();
    sqlx::query("INSERT INTO devices(id,tenant_id,name,code) VALUES($1,$2,'second cluster fixture',619438178)")
        .bind(second_device_id.as_uuid()).bind(account.personal_tenant_id.as_uuid()).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO endpoints(endpoint_key,tenant_id,owner_kind,user_id) VALUES($1,$2,'user',$3)",
    )
    .bind(second_operator.public().as_bytes().as_slice())
    .bind(account.personal_tenant_id.as_uuid())
    .bind(account.id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO endpoints(endpoint_key,tenant_id,owner_kind,device_id) VALUES($1,$2,'device',$3)")
        .bind(second_device.public().as_bytes().as_slice()).bind(account.personal_tenant_id.as_uuid()).bind(second_device_id.as_uuid()).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO endpoint_user_contexts(endpoint_key,token_hash,revision) VALUES($1,$2,1)",
    )
    .bind(second_operator.public().as_bytes().as_slice())
    .bind(&hash)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO device_connection_intents(operator_endpoint_key,device_id,expires_at) VALUES($1,$2,clock_timestamp()+interval '5 minutes')")
        .bind(second_operator.public().as_bytes().as_slice()).bind(second_device_id.as_uuid()).execute(&pool).await.unwrap();
    sqlx::query("UPDATE server_settings SET policy_revision=policy_revision+1")
        .execute(&pool)
        .await
        .unwrap();
    let second_config = || {
        PabEndpointConfig::new(vec![config.nodes[1].url.parse().unwrap()])
            .unwrap()
            .with_extra_ca_certificates(vec![certified.cert.der().clone()])
            .relay_only_for_testing()
    };
    let c = PabEndpoint::bind(second_config(), second_operator.clone())
        .await
        .unwrap();
    let d = PabEndpoint::bind(second_config(), second_device.clone())
        .await
        .unwrap();
    let c_control = supervisor(
        second_operator,
        EndpointProofPrincipal::User {
            user_id: account.id,
        },
    )
    .spawn();
    let d_control = supervisor(
        second_device,
        EndpointProofPrincipal::Device {
            device_id: second_device_id,
        },
    )
    .spawn();
    wait_home(&c, &config.nodes[1].url).await;
    wait_home(&d, &config.nodes[1].url).await;
    echo(&c, &d).await;
    let concurrent_started = std::time::Instant::now();
    tokio::join!(large_transfer(&a, &b, 512), large_transfer(&c, &d, 512));
    assert!(
        concurrent_started.elapsed() >= Duration::from_secs(50),
        "two nodes must not double one account's 10 Mbps budget"
    );
    eprintln!(
        "two-node concurrent 64 MiB, both application hashes matched, elapsed_ms={}",
        concurrent_started.elapsed().as_millis()
    );
    c.close().await;
    d.close().await;
    c_control.shutdown().await.unwrap();
    d_control.shutdown().await.unwrap();
    store
        .set_relay_state(
            "a",
            RelayAdminState::Maintenance,
            uuid::Uuid::new_v4(),
            account.id.as_uuid(),
            false,
            0,
            3,
        )
        .await
        .unwrap();
    relay_a.shutdown().await.unwrap();
    let switched = std::time::Instant::now();
    wait_home(&a, &config.nodes[1].url).await;
    wait_home(&b, &config.nodes[1].url).await;
    echo(&a, &b).await;
    assert!(switched.elapsed() < Duration::from_secs(30));
    let resolved = operator_control
        .device_network_resolver()
        .resolve(device_ref)
        .await
        .unwrap();
    assert_eq!(resolved.relay_urls, vec![config.nodes[1].url.clone()]);
    if three_nodes {
        // The first maintenance operation still owns the maintenance slot even
        // after the node goes offline. Another node cannot silently overlap it.
        assert!(matches!(
            store
                .set_relay_state(
                    "b",
                    RelayAdminState::Maintenance,
                    uuid::Uuid::new_v4(),
                    account.id.as_uuid(),
                    true,
                    0,
                    3
                )
                .await,
            Err(pab_server::StoreError::Conflict(_))
        ));
        store
            .set_relay_state(
                "a",
                RelayAdminState::Disabled,
                uuid::Uuid::new_v4(),
                account.id.as_uuid(),
                false,
                0,
                3,
            )
            .await
            .unwrap();
        store
            .set_relay_state(
                "c",
                RelayAdminState::Active,
                uuid::Uuid::new_v4(),
                account.id.as_uuid(),
                false,
                0,
                3,
            )
            .await
            .unwrap();
        store
            .set_relay_state(
                "b",
                RelayAdminState::Maintenance,
                uuid::Uuid::new_v4(),
                account.id.as_uuid(),
                false,
                0,
                3,
            )
            .await
            .unwrap();
        relay_b.take().unwrap().shutdown().await.unwrap();
        let second_switch = std::time::Instant::now();
        wait_home(&a, &config.nodes[2].url).await;
        wait_home(&b, &config.nodes[2].url).await;
        echo(&a, &b).await;
        assert!(second_switch.elapsed() < Duration::from_secs(30));
        let resolved = operator_control
            .device_network_resolver()
            .resolve(device_ref)
            .await
            .unwrap();
        assert_eq!(resolved.relay_urls, vec![config.nodes[2].url.clone()]);
        eprintln!(
            "three-node matrix: A -> B -> C, both Relay-only application echoes matched, second_switch_ms={}",
            second_switch.elapsed().as_millis()
        );
    }
    a.close().await;
    b.close().await;
    operator_control.shutdown().await.unwrap();
    device_control.shutdown().await.unwrap();
    network_task.abort();
    if let Some(relay) = relay_b {
        relay.shutdown().await.unwrap();
    }
    if let Some(relay) = relay_c {
        relay.shutdown().await.unwrap();
    }
    handle.shutdown();
    server.await.unwrap();
}
