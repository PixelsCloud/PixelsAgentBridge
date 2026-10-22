use super::*;
use pab_server::relay_cluster::{RelayClusterConfig, RelayNodeConfig};

async fn cluster(pool: PgPool) -> (Router, ControlPlane, String) {
    let control = ControlPlane::new(
        PostgresStore::from_pool(pool.clone()),
        PasswordPolicy::default(),
    )
    .unwrap();
    control.initialize_settings().await.unwrap();
    let control = control
        .with_relay_cluster(Some(RelayClusterConfig {
            heartbeat_seconds: 5,
            offline_seconds: 15,
            recovery_seconds: 0,
            maintenance_seconds: 30,
            nodes: [
                "healthy",
                "maintaining",
                "disabled",
                "incompatible",
                "expired",
                "unknown",
            ]
            .into_iter()
            .map(|name| RelayNodeConfig {
                node_id: name.into(),
                url: format!("https://{name}.example/"),
                region: "isolated".into(),
                weight: 100,
                capacity_mbps: 100,
                control_secret: format!("isolated-relay-{name}-01234567890123456789"),
            })
            .collect(),
        }))
        .await
        .unwrap();
    let app = pab_server::control_router(ControlApiState::new(
        control.clone(),
        ControlApiConfig::default(),
        RelayControlAuth::new("isolated-relay-management-01234567890123456789").unwrap(),
    ));
    let (status, cookie, _) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"snapshot-admin", "password":"isolated password long enough"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    pab_server::web::bootstrap_admin(&control, "snapshot-admin")
        .await
        .unwrap();
    let (_, _, snapshot) = call(&app, "GET", "/api/web/relays", &cookie, Value::Null).await;
    let instance = uuid::Uuid::parse_str(snapshot["server_instance"].as_str().unwrap()).unwrap();
    sqlx::query("UPDATE relay_cluster_nodes SET server_instance=$1")
        .bind(instance)
        .execute(&pool)
        .await
        .unwrap();
    (app, control, cookie)
}

#[sqlx::test(migrations = "./migrations")]
async fn overview_list_filters_and_pagination_share_cluster_classification(pool: PgPool) {
    let (app, _, cookie) = cluster(pool.clone()).await;
    // SQL-seeded heartbeats validate the read model; this is not a protocol probe.
    sqlx::query("UPDATE relay_cluster_nodes SET health='healthy',last_seen_at=clock_timestamp(),uploaded_bytes=9007199254740993")
        .execute(&pool).await.unwrap();
    for (node, state) in [("maintaining", "maintenance"), ("disabled", "disabled")] {
        sqlx::query("UPDATE relay_cluster_nodes SET admin_state=$2 WHERE node_id=$1")
            .bind(node)
            .bind(state)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE relay_cluster_nodes SET health='offline' WHERE node_id='disabled'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE relay_cluster_nodes SET health='incompatible' WHERE node_id='incompatible'",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE relay_cluster_nodes SET last_seen_at=clock_timestamp()-interval '20 seconds' WHERE node_id='expired'").execute(&pool).await.unwrap();
    sqlx::query("UPDATE relay_cluster_nodes SET last_seen_at=NULL,health='starting' WHERE node_id='unknown'").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO relay_cluster_nodes(node_id,public_url,weight,capacity_mbps,configured,health,last_seen_at) VALUES('historical','https://historical.example/',100,100,false,'healthy',clock_timestamp())").execute(&pool).await.unwrap();
    let (status, _, list) = call(
        &app,
        "GET",
        "/api/web/relays?page_size=2",
        &cookie,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["total"], 7);
    assert_eq!(list["items"].as_array().unwrap().len(), 2);
    let expected =
        json!({"configured":6,"available":1,"maintenance":1,"disabled":1,"abnormal":3,"history":1});
    assert_eq!(list["summary"], expected);
    assert_eq!(list["offline_seconds"], 15);
    assert_eq!(list["mode"], "cluster");
    assert_eq!(list["health"]["server"], "responding");
    assert_eq!(list["health"]["database"], "reachable");
    assert_eq!(list["health"]["source"], "management_database_query");
    assert_eq!(list["health"]["checked_at"], list["sampled_at"]);
    assert!(matches!(
        list["health"]["latency_grade"].as_str(),
        Some("normal" | "slow")
    ));
    assert!(list["sampled_at"].is_i64());
    let (status, _, overview) = call(
        &app,
        "GET",
        "/api/web/overview?scope=all",
        &cookie,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(overview["relays"], 6);
    assert_eq!(overview["relay_status"]["summary"], expected);
    for category in [
        "configured",
        "available",
        "maintenance",
        "disabled",
        "abnormal",
        "history",
    ] {
        let (status, _, filtered) = call(
            &app,
            "GET",
            &format!("/api/web/relays?status={category}"),
            &cookie,
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(filtered["total"], expected[category]);
        assert_eq!(filtered["summary"], expected);
        assert_eq!(
            filtered["items"].as_array().unwrap().len() as i64,
            expected[category].as_i64().unwrap()
        );
    }
    let (_, _, healthy) = call(
        &app,
        "GET",
        "/api/web/relays?status=available",
        &cookie,
        Value::Null,
    )
    .await;
    assert_eq!(
        healthy["items"][0]["uploaded_bytes_exact"],
        "9007199254740993"
    );
    let (_, _, expired) = call(
        &app,
        "GET",
        "/api/web/relays?q=expired",
        &cookie,
        Value::Null,
    )
    .await;
    assert_eq!(expired["items"][0]["health"], "offline");
    assert_eq!(expired["items"][0]["online"], false);
    for query in [
        "status=invalid",
        "page=0",
        "page_size=101",
        "unsupported=true",
    ] {
        assert_eq!(
            call(
                &app,
                "GET",
                &format!("/api/web/relays?{query}"),
                &cookie,
                Value::Null
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let (_, reader, _) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"snapshot-reader","password":"isolated password long enough"}),
    )
    .await;
    assert_eq!(
        call(&app, "GET", "/api/web/relays", &reader, Value::Null)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert!(
        call(&app, "GET", "/api/web/overview", &reader, Value::Null)
            .await
            .2
            .get("relay_status")
            .is_none()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn database_unavailable_returns_error_instead_of_zero_counts(pool: PgPool) {
    let (app, _, cookie) = cluster(pool.clone()).await;
    pool.close().await;
    for path in ["/api/web/relays", "/api/web/overview?scope=all"] {
        let (status, _, response) = call(&app, "GET", path, &cookie, Value::Null).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(response["code"], "server_error");
        assert!(response.get("summary").is_none());
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn server_restart_distrusts_old_samples_and_cannot_reactivate_old_nodes(pool: PgPool) {
    let (app, control, cookie) = cluster(pool.clone()).await;
    sqlx::query("UPDATE relay_cluster_nodes SET health='healthy',last_seen_at=clock_timestamp()")
        .execute(&pool)
        .await
        .unwrap();
    let id = uuid::Uuid::new_v4();
    let (status, _, _) = call(
        &app,
        "PATCH",
        "/api/web/relays/healthy/state",
        &cookie,
        json!({"state":"maintenance","operation_id":id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let restarted = pab_server::control_router(ControlApiState::new(
        control,
        ControlApiConfig::default(),
        RelayControlAuth::new("isolated-restarted-server-01234567890123456789").unwrap(),
    ));
    let (status, _, nodes) = call(&restarted, "GET", "/api/web/relays", &cookie, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(nodes["summary"]["available"], 0);
    assert!(
        nodes["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["health"] == "starting" && node["online"] == false)
    );
    let (_, _, operation) = call(
        &restarted,
        "GET",
        &format!("/api/web/relay-operations/{id}"),
        &cookie,
        Value::Null,
    )
    .await;
    assert_eq!(operation["phase"], "waiting");
    assert!(operation["remaining_capacity_mbps"].is_null());
    let (status, _, error) = call(
        &restarted,
        "PATCH",
        "/api/web/relays/healthy/state",
        &cookie,
        json!({"state":"active","operation_id":uuid::Uuid::new_v4()}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["code"], "relay_not_ready");
}

#[sqlx::test(migrations = "./migrations")]
async fn two_administrators_cannot_start_overlapping_maintenance(pool: PgPool) {
    let (app, _, first_cookie) = cluster(pool.clone()).await;
    let (_, second_cookie, second) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"second-operator","password":"isolated password long enough"}),
    )
    .await;
    let (status, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/web/accounts/{}", second["id"].as_str().unwrap()),
        &first_cookie,
        json!({"status":"active","server_admin":true,"revision":1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    sqlx::query("UPDATE relay_cluster_nodes SET health='healthy',last_seen_at=clock_timestamp()")
        .execute(&pool)
        .await
        .unwrap();
    let (first, second) = tokio::join!(
        call(
            &app,
            "PATCH",
            "/api/web/relays/healthy/state",
            &first_cookie,
            json!({"state":"maintenance","operation_id":uuid::Uuid::new_v4()})
        ),
        call(
            &app,
            "PATCH",
            "/api/web/relays/maintaining/state",
            &second_cookie,
            json!({"state":"maintenance","operation_id":uuid::Uuid::new_v4()})
        ),
    );
    let mut statuses = [first.0.as_u16(), second.0.as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 409]);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM relay_maintenance_operations WHERE phase IN ('waiting','ready')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM web_admin_events WHERE action='relay_maintenance'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(events, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn browser_operation_recovers_committed_request_with_same_id_and_failure_reason(
    pool: PgPool,
) {
    let (app, _, cookie) = cluster(pool.clone()).await;
    sqlx::query("UPDATE relay_cluster_nodes SET health='healthy',last_seen_at=clock_timestamp()")
        .execute(&pool)
        .await
        .unwrap();
    let id = uuid::Uuid::new_v4();
    let input = json!({"state":"maintenance","operation_id":id,"allow_outage":false});
    // Deliberately ignore the first response, then retry the same operation ID.
    assert_eq!(
        call(
            &app,
            "PATCH",
            "/api/web/relays/healthy/state",
            &cookie,
            input.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, _, result) = call(
        &app,
        "PATCH",
        "/api/web/relays/healthy/state",
        &cookie,
        input,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["id"], id.to_string());
    assert_eq!(result["phase"], "waiting");
    assert_eq!(result["remaining_capacity_mbps"], 500);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM web_admin_events WHERE resource_id=$1 AND action='relay_maintenance'",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let path = format!("/api/web/relay-operations/{id}");
    let (_, _, recovered) = call(&app, "GET", &path, &cookie, Value::Null).await;
    assert_eq!(recovered["actor"], "snapshot-admin");
    assert_eq!(recovered["deadline_unix_ms"], result["deadline_unix_ms"]);
    sqlx::query("UPDATE relay_maintenance_operations SET deadline_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(id).execute(&pool).await.unwrap();
    assert_eq!(
        call(&app, "GET", &path, &cookie, Value::Null).await.2["phase"],
        "ready"
    );
    sqlx::query("UPDATE relay_maintenance_operations SET deadline_at=clock_timestamp()-interval '6 minutes' WHERE id=$1").bind(id).execute(&pool).await.unwrap();
    let (status, _, failed) = call(&app, "GET", &path, &cookie, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(failed["phase"], "failed");
    assert_eq!(failed["error_code"], "operator_lease_expired");
    assert!(failed["completed_at_unix_ms"].is_i64());
    assert!(!failed.to_string().contains("isolated password"));
    assert_eq!(
        call(&app, "GET", &path, "", Value::Null).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn completed_resume_is_not_reverted_by_old_maintenance_reads_or_retries(pool: PgPool) {
    let (app, _, cookie) = cluster(pool.clone()).await;
    sqlx::query("UPDATE relay_cluster_nodes SET health='healthy',last_seen_at=clock_timestamp()")
        .execute(&pool).await.unwrap();
    let maintenance_id = uuid::Uuid::new_v4();
    let input = json!({"state":"maintenance","operation_id":maintenance_id,"allow_outage":false});
    let state_path = "/api/web/relays/healthy/state";
    assert_eq!(call(&app, "PATCH", state_path, &cookie, input.clone()).await.0, StatusCode::OK);
    let path = format!("/api/web/relay-operations/{maintenance_id}");
    sqlx::query("UPDATE relay_maintenance_operations SET deadline_at=clock_timestamp()-interval '6 minutes' WHERE id=$1")
        .bind(maintenance_id).execute(&pool).await.unwrap();
    let expired = call(&app, "GET", &path, &cookie, Value::Null).await.2;
    assert_eq!(expired["phase"], "failed");
    assert_eq!(expired["error_code"], "operator_lease_expired");
    let admin_state: String = sqlx::query_scalar("SELECT admin_state FROM relay_cluster_nodes WHERE node_id='healthy'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(admin_state, "maintenance");

    let resume_id = uuid::Uuid::new_v4();
    let (status, _, resumed) = call(&app, "PATCH", state_path, &cookie,
        json!({"state":"active","operation_id":resume_id,"allow_outage":false})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resumed["phase"], "completed");
    assert_eq!(resumed["action"], "activate");
    assert_eq!(call(&app, "GET", &path, &cookie, Value::Null).await.2["phase"], "failed");
    assert_eq!(call(&app, "PATCH", state_path, &cookie, input).await.2["id"], maintenance_id.to_string());
    let (_, _, list) = call(&app, "GET", "/api/web/relays", &cookie, Value::Null).await;
    let node = list["items"].as_array().unwrap().iter().find(|node| node["id"] == "healthy").unwrap();
    assert_eq!(node["admin_state"], "active");
    assert_eq!(node["operation_id"], resume_id.to_string());
    assert_eq!(node["available"], true);
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM web_admin_events WHERE action IN ('relay_maintenance','relay_activate')")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(events, 2);
}


#[sqlx::test(migrations = "./migrations")]
async fn capacity_edit_is_authorized_persistent_and_audited(pool: PgPool) {
    let (app, control, cookie) = cluster(pool.clone()).await;
    let endpoint = "/api/web/relays/healthy/capacity";
    let (_, user_cookie, _) = call(&app, "POST", "/api/web/register", "", json!({"username":"capacity-ordinary", "password":"isolated ordinary password"})).await;
    for (session, expected) in [("", StatusCode::UNAUTHORIZED), (user_cookie.as_str(), StatusCode::FORBIDDEN)] {
        assert_eq!(call(&app, "PATCH", endpoint, session, json!({"capacity_mbps":200})).await.0, expected);
    }
    for value in [json!(0),json!(-1),json!(1.5),json!(2147483648_u64),json!("200")] {
        let status = call(&app, "PATCH", endpoint, &cookie, json!({"capacity_mbps":value})).await.0;
        assert!(status.is_client_error());
    }
    assert_eq!(sqlx::query_scalar::<_,i32>("SELECT capacity_mbps FROM relay_cluster_nodes WHERE node_id='healthy'").fetch_one(&pool).await.unwrap(),100);
    assert_eq!(call(&app,"PATCH",endpoint,&cookie,json!({"capacity_mbps":200})).await.0,StatusCode::OK);
    let row = call(&app,"GET","/api/web/relays?q=healthy",&cookie,Value::Null).await.2;
    assert_eq!(row["items"][0]["capacity_mbps"],200);
    assert_eq!(row["items"][0]["capacity_mode"],"configured");
    assert!(row["items"][0]["ip_addresses"].is_array());
    let audit: (i64,i64) = sqlx::query_as("SELECT (before_values->>'mbps')::bigint,(after_values->>'mbps')::bigint FROM web_admin_events WHERE action='relay.capacity_updated' AND result='succeeded'").fetch_one(&pool).await.unwrap();
    assert_eq!(audit,(100,200));
    assert_eq!(call(&app,"PATCH",endpoint,&cookie,json!({"capacity_mbps":200,"machine_ip":"invalid-host"})).await.0,StatusCode::BAD_REQUEST);
    assert_eq!(call(&app,"PATCH",endpoint,&cookie,json!({"capacity_mbps":200,"machine_ip":"2001:db8::9"})).await.0,StatusCode::OK);
    let row = call(&app,"GET","/api/web/relays?q=healthy",&cookie,Value::Null).await.2;
    assert_eq!(row["items"][0]["ip_addresses"],json!(["2001:db8::9"]));
    assert_eq!(row["items"][0]["ip_source"],"configured");
    // Omitting the optional IP must preserve the administrator's saved value.
    assert_eq!(call(&app,"PATCH",endpoint,&cookie,json!({"capacity_mbps":200})).await.0,StatusCode::OK);
    // A service restart loads the old deployment value (100), then overlays the admin edit.
    let config = RelayClusterConfig {heartbeat_seconds:5, offline_seconds:15, recovery_seconds:0, maintenance_seconds:30, nodes:vec![RelayNodeConfig {node_id:"healthy".into(),url:"https://127.0.0.1/".into(),region:"isolated".into(),weight:100,capacity_mbps:100,control_secret:"isolated-capacity-restart-01234567890123456789".into()}]};
    let restarted = ControlPlane::new(PostgresStore::from_pool(pool.clone()),PasswordPolicy::default()).unwrap().with_relay_cluster(Some(config)).await.unwrap();
    let directory = restarted.store().relay_directory(15).await.unwrap();
    assert_eq!(directory.nodes.iter().find(|n| n.node_id=="healthy").unwrap().capacity_mbps,200);
    let files = tempfile::tempdir().unwrap();
    let file = files.path().join("server.toml");
    std::fs::write(&file, r#"listen = "127.0.0.1:0"
[database]
url = "postgres://unused/unused"
[tls]
cert = "cert.pem"
key = "key.pem"
[web]
origin = "https://web.example"
assets = "web"
[relay]
control_secret = "isolated-capacity-restart-01234567890123456789"
[relay.directory]
[[relay.directory.nodes]]
node_id = "healthy"
url = "https://127.0.0.1/"
capacity_mbps = 50
control_secret = "isolated-capacity-restart-01234567890123456789"
"#).unwrap();
    let restarted = restarted.with_relay_config_source(file, "isolated-capacity-restart-01234567890123456789");
    restarted.reload_relay_config().await.unwrap();
    let restarted_app = pab_server::control_router(ControlApiState::new(restarted,ControlApiConfig::default(),RelayControlAuth::new("isolated-capacity-restart-01234567890123456789").unwrap()));
    let row = call(&restarted_app,"GET","/api/web/relays?q=healthy",&cookie,Value::Null).await.2;
    assert_eq!(row["items"][0]["ip_addresses"],json!(["2001:db8::9"]));
    assert_eq!(call(&restarted_app,"PATCH",endpoint,&cookie,json!({"capacity_mbps":200,"machine_ip":""})).await.0,StatusCode::OK);
    let row = call(&restarted_app,"GET","/api/web/relays?q=healthy",&cookie,Value::Null).await.2;
    assert_eq!(row["items"][0]["ip_addresses"],json!(["127.0.0.1"]));
    assert_eq!(row["items"][0]["capacity_mbps"],200);
    assert_eq!(call(&app,"PATCH","/api/web/relays/missing/capacity",&cookie,json!({"capacity_mbps":200})).await.0,StatusCode::NOT_FOUND);
    drop(control);
}
