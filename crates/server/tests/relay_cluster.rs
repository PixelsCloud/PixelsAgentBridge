use pab_protocol::{RelayAdminState, RelayHealth};
use pab_server::{
    PostgresStore,
    relay_cluster::{RelayClusterConfig, RelayNodeConfig},
};
use sqlx::PgPool;

fn config() -> RelayClusterConfig {
    RelayClusterConfig {
        heartbeat_seconds: 5,
        offline_seconds: 15,
        recovery_seconds: 0,
        maintenance_seconds: 30,
        nodes: vec![
            RelayNodeConfig {
                node_id: "windows-a".into(),
                url: "https://a.example/".into(),
                region: "test".into(),
                weight: 100,
                capacity_mbps: 100,
                control_secret: "isolated-test-node-a-01234567890123456789".into(),
            },
            RelayNodeConfig {
                node_id: "linux-b".into(),
                url: "https://b.example/".into(),
                region: "test".into(),
                weight: 100,
                capacity_mbps: 100,
                control_secret: "isolated-test-node-b-01234567890123456789".into(),
            },
        ],
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn one_host_bandwidth_node_uses_directory_budgets_and_survives_disabled_restart(
    pool: PgPool,
) {
    let store = PostgresStore::from_pool(pool.clone());
    store.initialize_settings().await.unwrap();
    let mut directory = config();
    directory.nodes.truncate(1);
    directory.nodes[0].capacity_mbps = 0;
    directory.validate().unwrap();
    store.configure_relay_cluster(&directory).await.unwrap();
    store.relay_directory(15).await.unwrap().validate().unwrap();
    let control =
        pab_server::ControlPlane::new(store.clone(), pab_server::PasswordPolicy::default())
            .unwrap();
    let user = control
        .register_account("single-host", "isolated single host password")
        .await
        .unwrap();
    let boot = uuid::Uuid::new_v4();
    let instance = uuid::Uuid::new_v4();
    store
        .register_relay_boot("windows-a", boot, "test", true, &directory, instance)
        .await
        .unwrap();
    for _ in 0..2 {
        store
            .heartbeat_relay("windows-a", boot, true, &directory)
            .await
            .unwrap();
    }
    let budget = store
        .relay_budgets("windows-a", boot, &[user.id], &directory)
        .await
        .unwrap();
    assert_eq!(budget.node_capacity_bytes_per_second, 0);
    assert_eq!(budget.grants[0].bytes_per_second, 1_250_000);
    assert!(
        store
            .set_relay_state(
                "windows-a",
                RelayAdminState::Maintenance,
                uuid::Uuid::new_v4(),
                user.id.as_uuid(),
                false,
                0,
                15
            )
            .await
            .is_err()
    );
    store
        .set_relay_state(
            "windows-a",
            RelayAdminState::Disabled,
            uuid::Uuid::new_v4(),
            user.id.as_uuid(),
            true,
            0,
            15,
        )
        .await
        .unwrap();
    store.retire_relay("windows-a", boot).await.unwrap();
    let replacement = uuid::Uuid::new_v4();
    store
        .register_relay_boot("windows-a", replacement, "test", true, &directory, instance)
        .await
        .unwrap();
    for _ in 0..2 {
        store
            .heartbeat_relay("windows-a", replacement, true, &directory)
            .await
            .unwrap();
    }
    assert!(
        store
            .relay_budgets("windows-a", replacement, &[user.id], &directory)
            .await
            .is_err()
    );
    store
        .set_relay_state(
            "windows-a",
            RelayAdminState::Active,
            uuid::Uuid::new_v4(),
            user.id.as_uuid(),
            false,
            0,
            15,
        )
        .await
        .unwrap();
    assert_eq!(
        store.relay_directory(15).await.unwrap().nodes[0].admin_state,
        RelayAdminState::Active
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn legacy_toml_normalizes_registered_identity_without_changing_secret_or_history(
    pool: PgPool,
) {
    let store = PostgresStore::from_pool(pool.clone());
    sqlx::query("INSERT INTO relay_nodes(node_id,public_url,offered_policy_version,server_instance,last_seen_at) VALUES('old-node','https://relay.example/',1,gen_random_uuid(),clock_timestamp()-interval '70 seconds')").execute(&pool).await.unwrap();
    let legacy = pab_server::config::RelayConfig {
        control_secret: "old-shared-secret-retained-01234567890123456789".into(),
        cluster: None,
    };
    let directory = legacy.resolved_directory(&store).await.unwrap().unwrap();
    assert_eq!(directory.nodes[0].node_id, "old-node");
    assert_eq!(directory.nodes[0].control_secret, legacy.control_secret);
    assert_eq!(directory.nodes[0].capacity_mbps, 0);
    store.configure_relay_cluster(&directory).await.unwrap();
    let old_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM relay_nodes WHERE node_id='old-node'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(old_rows, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn directory_readers_do_not_wait_for_writers_and_keep_revision_consistent(pool: PgPool) {
    let store = PostgresStore::from_pool(pool.clone());
    store.configure_relay_cluster(&config()).await.unwrap();
    let before = store.relay_directory(15).await.unwrap();
    let mut writer = pool.begin().await.unwrap();
    sqlx::query("SELECT revision FROM relay_cluster_state WHERE singleton FOR UPDATE")
        .fetch_one(&mut *writer)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE relay_cluster_nodes SET admin_state='maintenance' WHERE node_id='windows-a'",
    )
    .execute(&mut *writer)
    .await
    .unwrap();
    sqlx::query("UPDATE relay_cluster_state SET revision=revision+1 WHERE singleton")
        .execute(&mut *writer)
        .await
        .unwrap();
    let mut readers = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let store = store.clone();
        let revision = before.revision;
        readers.spawn(async move {
            let directory = store.relay_directory(15).await.unwrap();
            assert_eq!(directory.revision, revision);
            assert_eq!(
                directory
                    .nodes
                    .iter()
                    .find(|n| n.node_id == "windows-a")
                    .unwrap()
                    .admin_state,
                RelayAdminState::Active
            );
        });
    }
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while let Some(result) = readers.join_next().await {
            result.unwrap();
        }
    })
    .await
    .expect("directory reads cannot queue behind a held catalog write lock");
    writer.commit().await.unwrap();
    let after = store.relay_directory(15).await.unwrap();
    assert_eq!(after.revision, before.revision + 1);
    assert_eq!(
        after
            .nodes
            .iter()
            .find(|n| n.node_id == "windows-a")
            .unwrap()
            .admin_state,
        RelayAdminState::Maintenance
    );
    sqlx::query("UPDATE relay_cluster_nodes SET configured=false")
        .execute(&pool)
        .await
        .unwrap();
    let empty = store.relay_directory(15).await.unwrap();
    assert!(empty.nodes.is_empty());
    assert_eq!(empty.revision, after.revision);
}

#[sqlx::test(migrations = "./migrations")]
async fn live_reload_is_atomic_and_publishes_rotated_node_credentials(pool: PgPool) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("server.toml");
    let valid = include_str!("../../../packaging/server/cluster.example.toml");
    std::fs::write(&path, valid).unwrap();
    let loaded = pab_server::config::ServerConfig::load(&path).unwrap();
    let initial = loaded.relay.cluster.clone().unwrap();
    let store = PostgresStore::from_pool(pool.clone());
    store.initialize_settings().await.unwrap();
    let control =
        pab_server::ControlPlane::new(store.clone(), pab_server::PasswordPolicy::default())
            .unwrap()
            .with_relay_cluster(Some(initial.clone()))
            .await
            .unwrap()
            .with_relay_config_source(path.clone(), &loaded.relay.control_secret);
    let account = control
        .register_account("reload-test", "isolated reload test password")
        .await
        .unwrap();
    store
        .set_relay_state(
            "linux-a",
            RelayAdminState::Maintenance,
            uuid::Uuid::new_v4(),
            account.id.as_uuid(),
            true,
            0,
            15,
        )
        .await
        .unwrap();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let tls = axum_server::tls_rustls::RustlsConfig::from_der(
        vec![certificate.cert.der().to_vec()],
        certificate.signing_key.serialize_der(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!(
        "wss://localhost:{}/relay-control",
        listener.local_addr().unwrap().port()
    );
    let handle = axum_server::Handle::new();
    let stop = handle.clone();
    let state = pab_server::ControlApiState::new(
        control.clone(),
        pab_server::ControlApiConfig::default(),
        pab_server::RelayControlAuth::new(&loaded.relay.control_secret)
            .unwrap()
            .with_cluster(Some(&initial)),
    );
    let server = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .unwrap()
            .handle(stop)
            .serve(pab_server::control_router(state).into_make_service())
            .await
            .unwrap()
    });
    let connector = pab_agent_core::tls_connector(Some(certificate.cert.pem().as_bytes())).unwrap();
    let connect = |secret: String| {
        let url = url.clone();
        let connector = connector.clone();
        async move { pab_relay::RelayControlClient::connect(&url, &secret, connector).await }
    };
    let before = store.relay_directory(15).await.unwrap();
    std::fs::write(
        &path,
        valid.replace(
            "https://relay-a.example:31443",
            "http://relay-a.example:31443",
        ),
    )
    .unwrap();
    assert!(control.reload_relay_config().await.is_err());
    assert_eq!(store.relay_directory(15).await.unwrap(), before);
    assert!(
        connect(initial.nodes[0].control_secret.clone())
            .await
            .is_ok()
    );
    let rotated = "rotated-linux-node-secret-01234567890123456789";
    std::fs::write(
        &path,
        valid.replace(&initial.nodes[0].control_secret, rotated),
    )
    .unwrap();
    control.reload_relay_config().await.unwrap();
    // The listener was built with the original auth map. It must still consult
    // the atomically reloaded credentials rather than that captured map.
    assert!(
        connect(initial.nodes[0].control_secret.clone())
            .await
            .is_err()
    );
    assert!(connect(rotated.into()).await.is_ok());
    let after = store.relay_directory(15).await.unwrap();
    assert!(after.revision > before.revision);
    assert_eq!(after.nodes[0].admin_state, RelayAdminState::Maintenance);
    std::fs::write(
        &path,
        valid.replace(
            &loaded.relay.control_secret,
            "changed-legacy-secret-01234567890123456789",
        ),
    )
    .unwrap();
    assert!(control.reload_relay_config().await.is_err());
    assert!(connect(rotated.into()).await.is_ok());
    handle.shutdown();
    server.await.unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn catalog_health_boot_fencing_and_expiry_are_persistent(pool: PgPool) {
    let store = PostgresStore::from_pool(pool.clone());
    let config = config();
    store.configure_relay_cluster(&config).await.unwrap();
    let first = store.relay_directory(15).await.unwrap();
    assert_eq!(first.nodes.len(), 2);
    assert!(first.nodes.iter().all(|n| !n.accepts_new_connections()));
    let boot = uuid::Uuid::new_v4();
    let instance = uuid::Uuid::new_v4();
    store
        .register_relay_boot("windows-a", boot, "test-v1", true, &config, instance)
        .await
        .unwrap();
    assert!(
        store
            .register_relay_boot(
                "windows-a",
                uuid::Uuid::new_v4(),
                "test-v1",
                true,
                &config,
                instance
            )
            .await
            .is_err()
    );
    store
        .heartbeat_relay("windows-a", boot, true, &config)
        .await
        .unwrap();
    store
        .heartbeat_relay("windows-a", boot, true, &config)
        .await
        .unwrap();
    let healthy = store.relay_directory(15).await.unwrap();
    assert!(healthy.revision > first.revision);
    assert!(healthy.nodes[0].health == RelayHealth::Starting); // Linux B sorts first.
    assert_eq!(healthy.nodes[1].health, RelayHealth::Healthy);
    assert!(healthy.nodes[1].accepts_new_connections());
    // Reconnection of the SAME process (e.g. its usage socket) must not reset health.
    store
        .register_relay_boot("windows-a", boot, "test-v1", true, &config, instance)
        .await
        .unwrap();
    assert_eq!(
        store.relay_directory(15).await.unwrap().nodes[1].health,
        RelayHealth::Healthy
    );
    sqlx::query("UPDATE relay_cluster_nodes SET last_seen_at=clock_timestamp()-interval '16 seconds' WHERE node_id='windows-a'").execute(&pool).await.unwrap();
    let offline = store.relay_directory(15).await.unwrap();
    assert!(offline.revision > healthy.revision);
    assert_eq!(offline.nodes[1].health, RelayHealth::Offline);
    let replacement = uuid::Uuid::new_v4();
    store
        .register_relay_boot("windows-a", replacement, "test-v2", true, &config, instance)
        .await
        .unwrap();
    assert!(
        store
            .heartbeat_relay("windows-a", boot, true, &config)
            .await
            .is_err()
    );
    store
        .heartbeat_relay("windows-a", replacement, true, &config)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn config_reload_preserves_maintenance_intent_and_removes_old_nodes(pool: PgPool) {
    let store = PostgresStore::from_pool(pool.clone());
    let mut config = config();
    store.configure_relay_cluster(&config).await.unwrap();
    sqlx::query(
        "UPDATE relay_cluster_nodes SET admin_state='maintenance' WHERE node_id='windows-a'",
    )
    .execute(&pool)
    .await
    .unwrap();
    store.configure_relay_cluster(&config).await.unwrap();
    assert_eq!(
        store.relay_directory(15).await.unwrap().nodes[1].admin_state,
        RelayAdminState::Maintenance
    );
    config.nodes.remove(1);
    store.configure_relay_cluster(&config).await.unwrap();
    let directory = store.relay_directory(15).await.unwrap();
    assert_eq!(directory.nodes.len(), 1);
    assert_eq!(directory.nodes[0].node_id, "windows-a");
    assert_eq!(directory.nodes[0].admin_state, RelayAdminState::Maintenance);
    let configured: bool =
        sqlx::query_scalar("SELECT configured FROM relay_cluster_nodes WHERE node_id='linux-b'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!configured);
}

#[sqlx::test(migrations = "./migrations")]
async fn account_budget_is_reserved_across_nodes_restarts_and_concurrent_calls(pool: PgPool) {
    let store = PostgresStore::from_pool(pool.clone());
    let mut config = config();
    for node in &mut config.nodes {
        node.capacity_mbps = 0;
    }
    store.initialize_settings().await.unwrap();
    store.configure_relay_cluster(&config).await.unwrap();
    let control =
        pab_server::ControlPlane::new(store.clone(), pab_server::PasswordPolicy::default())
            .unwrap();
    let account = control
        .register_account("budget-fixture", "isolated password long enough")
        .await
        .unwrap();
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    let instance = uuid::Uuid::new_v4();
    for (node, boot) in [("windows-a", a), ("linux-b", b)] {
        store
            .register_relay_boot(node, boot, "test", true, &config, instance)
            .await
            .unwrap();
        store
            .heartbeat_relay(node, boot, true, &config)
            .await
            .unwrap();
        store
            .heartbeat_relay(node, boot, true, &config)
            .await
            .unwrap();
    }
    let users = [account.id];
    let first = store
        .relay_budgets("windows-a", a, &users, &config)
        .await
        .unwrap();
    assert_eq!(first.grants.len(), 1);
    assert_eq!(first.grants[0].bytes_per_second, 1_250_000);
    let waiting = store
        .relay_budgets("linux-b", b, &users, &config)
        .await
        .unwrap();
    assert!(waiting.grants.is_empty()); // A's lease is still reserved, including burst.
    let restarted = PostgresStore::from_pool(pool.clone());
    let waiting = restarted
        .relay_budgets("linux-b", b, &users, &config)
        .await
        .unwrap();
    assert!(waiting.grants.is_empty());
    // Simulate expiry without a 15-second wall-clock sleep.
    sqlx::query("UPDATE relay_budget_leases SET expires_at=clock_timestamp()-interval '1 second' WHERE user_id=$1")
        .bind(account.id.as_uuid()).execute(&pool).await.unwrap();
    let (one, two) = tokio::join!(
        store.relay_budgets("windows-a", a, &users, &config),
        restarted.relay_budgets("linux-b", b, &users, &config)
    );
    let one = one.unwrap();
    let two = two.unwrap();
    assert_eq!(
        one.grants[0].bytes_per_second + two.grants[0].bytes_per_second,
        1_250_000
    );
    assert_eq!(
        one.grants[0].burst_bytes + two.grants[0].burst_bytes,
        125_000
    );
    assert_eq!(one.grants[0].bytes_per_second, 625_000);
    assert!(one.grants[0].generation > first.grants[0].generation);
    // An unchanged lease is never extended by repeated heartbeat/query calls.
    let again = store
        .relay_budgets("windows-a", a, &users, &config)
        .await
        .unwrap();
    assert_eq!(again.grants[0], one.grants[0]);
    assert!(
        store
            .relay_budgets("windows-a", uuid::Uuid::new_v4(), &users, &config)
            .await
            .is_err()
    );
    assert!(
        store
            .relay_budgets("windows-a", a, &[account.id, account.id], &config)
            .await
            .is_err()
    );
    // A planned stop retires only its exact process. It cannot reconnect or
    // obtain a new lease, while the next version need not wait for offline TTL.
    store.retire_relay("windows-a", a).await.unwrap();
    assert!(
        store
            .heartbeat_relay("windows-a", a, true, &config)
            .await
            .is_err()
    );
    assert!(
        store
            .register_relay_boot("windows-a", a, "stale", true, &config, instance)
            .await
            .is_err()
    );
    assert!(
        store
            .relay_budgets("windows-a", a, &users, &config)
            .await
            .is_err()
    );
    let next = uuid::Uuid::new_v4();
    store
        .register_relay_boot("windows-a", next, "new", true, &config, instance)
        .await
        .unwrap();
    assert!(store.retire_relay("windows-a", a).await.is_err());
    store
        .heartbeat_relay("windows-a", next, true, &config)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn full_budget_batches_reuse_leases_without_refilling_or_per_account_refresh_transactions(
    pool: PgPool,
) {
    let store = PostgresStore::from_pool(pool.clone());
    let config = config();
    store.initialize_settings().await.unwrap();
    store.configure_relay_cluster(&config).await.unwrap();
    let boot = uuid::Uuid::new_v4();
    store
        .register_relay_boot(
            "windows-a",
            boot,
            "load-test",
            true,
            &config,
            uuid::Uuid::new_v4(),
        )
        .await
        .unwrap();
    store
        .heartbeat_relay("windows-a", boot, true, &config)
        .await
        .unwrap();
    store
        .heartbeat_relay("windows-a", boot, true, &config)
        .await
        .unwrap();
    let ids:Vec<uuid::Uuid>=sqlx::query_scalar("INSERT INTO users(id,username,username_key,password_hash,relay_limit_mbps) SELECT gen_random_uuid(),'budget-'||n,'budget-'||n,'fixture-not-a-valid-password-hash',10 FROM generate_series(1,256) n RETURNING id")
        .fetch_all(&pool).await.unwrap();
    let users = ids
        .iter()
        .copied()
        .map(pab_protocol::UserId::from_uuid)
        .collect::<Vec<_>>();
    let first = store
        .relay_budgets("windows-a", boot, &users, &config)
        .await
        .unwrap();
    assert_eq!(first.grants.len(), 256);
    let started = std::time::Instant::now();
    for _ in 0..10 {
        let next = store
            .relay_budgets("windows-a", boot, &users, &config)
            .await
            .unwrap();
        assert_eq!(next.grants, first.grants);
        assert!(next.revision > first.revision);
    }
    eprintln!(
        "256-account budget batch confirmed 10 times, elapsed_ms={}",
        started.elapsed().as_millis()
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    sqlx::query("UPDATE users SET status='disabled' WHERE id=$1")
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
    let next = store
        .relay_budgets("windows-a", boot, &users, &config)
        .await
        .unwrap();
    assert_eq!(next.grants.len(), 255);
    assert!(next.grants.iter().all(|grant| grant.user_id != users[0]));
}

#[sqlx::test(migrations = "./migrations")]
async fn maintenance_is_idempotent_bounded_and_abandoned_lock_is_recoverable(pool: PgPool) {
    let store = PostgresStore::from_pool(pool.clone());
    let config = config();
    store.initialize_settings().await.unwrap();
    store.configure_relay_cluster(&config).await.unwrap();
    let control =
        pab_server::ControlPlane::new(store.clone(), pab_server::PasswordPolicy::default())
            .unwrap();
    let account = control
        .register_account("maintenance-test", "isolated password long enough")
        .await
        .unwrap();
    sqlx::query("UPDATE relay_cluster_nodes SET health='healthy',last_seen_at=clock_timestamp()")
        .execute(&pool)
        .await
        .unwrap();
    let operation = uuid::Uuid::new_v4();
    let first = store
        .set_relay_state(
            "windows-a",
            RelayAdminState::Maintenance,
            operation,
            account.id.as_uuid(),
            false,
            0,
            15,
        )
        .await
        .unwrap();
    assert_eq!(first.remaining_capacity_mbps, Some(100));
    let again = store
        .set_relay_state(
            "windows-a",
            RelayAdminState::Maintenance,
            operation,
            account.id.as_uuid(),
            false,
            0,
            15,
        )
        .await
        .unwrap();
    assert_eq!(again.id, first.id);
    assert_eq!(again.phase, "ready");
    assert_eq!(again.deadline_unix_ms, first.deadline_unix_ms);
    assert_eq!(again.remaining_capacity_mbps, Some(100));
    assert_eq!(again.actor, "maintenance-test");
    assert!(again.server_now_unix_ms >= again.started_at_unix_ms);
    assert!(
        store
            .set_relay_state(
                "linux-b",
                RelayAdminState::Maintenance,
                uuid::Uuid::new_v4(),
                account.id.as_uuid(),
                true,
                0,
                15
            )
            .await
            .is_err()
    );
    assert!(
        store
            .set_relay_state(
                "windows-a",
                RelayAdminState::Disabled,
                operation,
                account.id.as_uuid(),
                false,
                0,
                15
            )
            .await
            .is_err()
    );
    store
        .set_relay_state(
            "windows-a",
            RelayAdminState::Active,
            uuid::Uuid::new_v4(),
            account.id.as_uuid(),
            false,
            0,
            15,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .relay_maintenance_operation(operation)
            .await
            .unwrap()
            .phase,
        "completed"
    );
    store
        .set_relay_state(
            "windows-a",
            RelayAdminState::Disabled,
            uuid::Uuid::new_v4(),
            account.id.as_uuid(),
            false,
            0,
            15,
        )
        .await
        .unwrap();
    assert!(
        store
            .set_relay_state(
                "linux-b",
                RelayAdminState::Maintenance,
                uuid::Uuid::new_v4(),
                account.id.as_uuid(),
                false,
                0,
                15
            )
            .await
            .is_err()
    );
    let abandoned = uuid::Uuid::new_v4();
    store
        .set_relay_state(
            "linux-b",
            RelayAdminState::Maintenance,
            abandoned,
            account.id.as_uuid(),
            true,
            0,
            15,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE relay_maintenance_operations SET deadline_at=clock_timestamp()-interval '6 minutes' WHERE id=$1").bind(abandoned).execute(&pool).await.unwrap();
    store
        .set_relay_state(
            "windows-a",
            RelayAdminState::Active,
            uuid::Uuid::new_v4(),
            account.id.as_uuid(),
            false,
            0,
            15,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .relay_maintenance_operation(abandoned)
            .await
            .unwrap()
            .phase,
        "failed"
    );
    assert_eq!(
        store.relay_directory(15).await.unwrap().nodes[0].admin_state,
        RelayAdminState::Maintenance
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn maintenance_poll_expires_without_another_mutation_and_samples_live_capacity(pool: PgPool) {
    let store = PostgresStore::from_pool(pool.clone());
    store.initialize_settings().await.unwrap();
    store.configure_relay_cluster(&config()).await.unwrap();
    let control =
        pab_server::ControlPlane::new(store.clone(), pab_server::PasswordPolicy::default())
            .unwrap();
    let account = control
        .register_account("operation-samples", "isolated password long enough")
        .await
        .unwrap();
    sqlx::query("UPDATE relay_cluster_nodes SET health='healthy',last_seen_at=clock_timestamp()")
        .execute(&pool)
        .await
        .unwrap();
    let id = uuid::Uuid::new_v4();
    let waiting = store
        .set_relay_state(
            "windows-a",
            RelayAdminState::Maintenance,
            id,
            account.id.as_uuid(),
            false,
            30,
            15,
        )
        .await
        .unwrap();
    assert_eq!(waiting.phase, "waiting");
    assert_eq!(waiting.remaining_capacity_mbps, Some(100));
    sqlx::query("UPDATE relay_cluster_nodes SET last_seen_at=clock_timestamp()-interval '20 seconds' WHERE node_id='linux-b'")
        .execute(&pool).await.unwrap();
    let stale = store.relay_maintenance_operation(id).await.unwrap();
    assert_eq!(stale.remaining_capacity_mbps, Some(0));
    assert!(stale.capacity_sampled_at_unix_ms >= waiting.capacity_sampled_at_unix_ms);
    sqlx::query("UPDATE relay_maintenance_operations SET deadline_at=clock_timestamp()-interval '6 minutes' WHERE id=$1")
        .bind(id).execute(&pool).await.unwrap();
    let failed = store.relay_maintenance_operation(id).await.unwrap();
    assert_eq!(failed.phase, "failed");
    assert_eq!(failed.error_code.as_deref(), Some("operator_lease_expired"));
    assert!(failed.completed_at_unix_ms.is_some());
    let state: String =
        sqlx::query_scalar("SELECT admin_state FROM relay_cluster_nodes WHERE node_id='windows-a'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "maintenance");
    // Operations created before migration 0008 must not fabricate zero capacity.
    sqlx::query("UPDATE relay_maintenance_operations SET offline_seconds=NULL WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .relay_maintenance_operation(id)
            .await
            .unwrap()
            .remaining_capacity_mbps,
        None
    );
    assert!(matches!(
        store
            .relay_maintenance_operation(uuid::Uuid::new_v4())
            .await,
        Err(pab_server::StoreError::NotFound)
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn automation_management_requires_admin_bearer_and_never_accepts_cookie_fallback(
    pool: PgPool,
) {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;
    async fn call(
        app: &Router,
        method: &str,
        path: &str,
        token: Option<&str>,
        cookie: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("Content-Type", "application/json");
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        if let Some(cookie) = cookie {
            request = request.header("Cookie", cookie);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
    let store = PostgresStore::from_pool(pool.clone());
    store.initialize_settings().await.unwrap();
    let control = pab_server::ControlPlane::new(store, pab_server::PasswordPolicy::default())
        .unwrap()
        .with_relay_cluster(Some(config()))
        .await
        .unwrap();
    let admin_account = control
        .register_account("relay-admin", "isolated password long enough")
        .await
        .unwrap();
    control
        .register_account("relay-reader", "isolated password long enough")
        .await
        .unwrap();
    pab_server::web::bootstrap_admin(&control, "relay-admin")
        .await
        .unwrap();
    let app = pab_server::control_router(pab_server::ControlApiState::new(
        control,
        pab_server::ControlApiConfig::default(),
        pab_server::RelayControlAuth::new("isolated-legacy-control-01234567890123456789").unwrap(),
    ));
    let (status, admin) = call(
        &app,
        "POST",
        "/api/account/session",
        None,
        None,
        json!({"username":"relay-admin","password":"isolated password long enough"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token = admin["access_token"].as_str().unwrap();
    let (_, reader) = call(
        &app,
        "POST",
        "/api/account/session",
        None,
        None,
        json!({"username":"relay-reader","password":"isolated password long enough"}),
    )
    .await;
    let cookie = format!("__Host-pab_session={token}");
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/admin/relays",
            None,
            Some(&cookie),
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/admin/relays",
            reader["access_token"].as_str(),
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, nodes) = call(
        &app,
        "GET",
        "/api/admin/relays",
        Some(token),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(nodes["items"].as_array().unwrap().len(), 2);
    assert!(!nodes.to_string().contains("isolated-test-node"));
    let operation = uuid::Uuid::new_v4();
    let body = json!({"state":"maintenance","operation_id":operation,"allow_outage":false});
    let (status, result) = call(
        &app,
        "PATCH",
        "/api/admin/relays/windows-a/state",
        Some(token),
        None,
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(result["code"], "relay_no_backup");
    let (status, result) = call(
        &app,
        "PATCH",
        "/api/admin/relays/windows-a/state",
        Some(token),
        None,
        json!({"state":"maintenance","operation_id":operation,"allow_outage":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["id"], operation.to_string());
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM web_admin_events WHERE action='relay_maintenance'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let first = pab_protocol::EndpointKey::new(
        *iroh_base::SecretKey::from_bytes(&[87; 32])
            .public()
            .as_bytes(),
    );
    let second = pab_protocol::EndpointKey::new(
        *iroh_base::SecretKey::from_bytes(&[88; 32])
            .public()
            .as_bytes(),
    );
    let pair = json!({"first":first,"second":second});
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/admin/relays/windows-a/probe",
            None,
            Some(&cookie),
            pair.clone()
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/admin/relays/windows-a/probe",
            reader["access_token"].as_str(),
            None,
            pair.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let store = PostgresStore::from_pool(pool.clone());
    let config = config();
    let boot = uuid::Uuid::new_v4();
    store
        .register_relay_boot(
            "windows-a",
            boot,
            "probe-test",
            true,
            &config,
            uuid::Uuid::new_v4(),
        )
        .await
        .unwrap();
    store
        .heartbeat_relay("windows-a", boot, true, &config)
        .await
        .unwrap();
    store
        .heartbeat_relay("windows-a", boot, true, &config)
        .await
        .unwrap();
    let (status, receipt) = call(
        &app,
        "POST",
        "/api/admin/relays/windows-a/probe",
        Some(token),
        None,
        pair.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/admin/relays/windows-a/probe",
            Some(token),
            None,
            pair.clone()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let id = receipt["probe_id"].as_str().unwrap();
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/api/admin/relay-probes/{id}"),
            Some(token),
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    sqlx::query(
        "INSERT INTO endpoints(endpoint_key,tenant_id,owner_kind,user_id) VALUES($1,$2,'user',$3)",
    )
    .bind(first.as_bytes().as_slice())
    .bind(admin_account.personal_tenant_id.as_uuid())
    .bind(admin_account.id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let (status, body) = call(
        &app,
        "POST",
        "/api/admin/relays/windows-a/probe",
        Some(token),
        None,
        pair,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "probe_endpoint_registered");
}
