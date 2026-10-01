//! Real stdio subprocess regression, using the MCP SDK and isolated storage.
//! This is transport testing, not a replacement for installed-host acceptance.
use pab_bridge::BridgeLocalStore;
use pab_protocol::{DeploymentId, DeviceId, DeviceRef, RequestId, TenantId};
use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
    service::RunningService,
};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};

type Client = RunningService<RoleClient, ()>;

async fn start(root: &Path, database: &Path, port: u16) -> (Client, tokio::process::Child) {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_pab-mcp"))
        .env("PAB_DATA_DIR", root)
        .env("PAB_BRIDGE_DATABASE", database)
        .env("PAB_MCP_GUEST", "1")
        .env("PAB_CONTROL_URL", format!("wss://127.0.0.1:{port}"))
        .env("PAB_DEPLOYMENT_ID", DeploymentId::from_u128(1).to_string())
        .env("PAB_RELAY_URLS", "https://127.0.0.1:1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    let client = tokio::time::timeout(Duration::from_secs(5), ().serve((stdout, stdin)))
        .await
        .unwrap()
        .unwrap();
    (client, child)
}

async fn call(client: &Client, name: &str, args: Value) -> CallToolResult {
    tokio::time::timeout(
        Duration::from_secs(2),
        client.call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        ),
    )
    .await
    .expect("tool was blocked behind network I/O")
    .unwrap()
}

fn result(response: CallToolResult) -> Value {
    assert_ne!(response.is_error, Some(true), "{:?}", response.content);
    response
        .structured_content
        .expect("missing structured result")
}

async fn stop(client: Client, mut child: tokio::process::Child) {
    client.cancel().await.unwrap();
    let status = tokio::time::timeout(Duration::from_secs(8), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success(), "MCP did not shut down cleanly: {status}");
}

#[tokio::test]
async fn offline_submission_query_cancel_dedup_and_two_sessions_use_real_stdio() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("bridge.sqlite3");
    let _store = BridgeLocalStore::open(&database).await.unwrap();
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(&database),
    )
    .await
    .unwrap();
    let device = DeviceRef {
        deployment_id: DeploymentId::from_u128(1),
        tenant_id: TenantId::from_u128(2),
        device_id: DeviceId::from_u128(3),
    };
    let os = if cfg!(windows) { "windows" } else { "linux" };
    sqlx::query("INSERT INTO remembered_devices (device_ref_json,device_code,alias,os_family_json,os_reminder) VALUES (?, '123456789', 'fixture', ?, 'cached fixture OS')")
        .bind(serde_json::to_string(&device).unwrap()).bind(format!("\"{os}\"" )).execute(&pool).await.unwrap();
    let source = directory.path().join("source.bin");
    tokio::fs::write(&source, b"fixture").await.unwrap();
    // Accept TCP but hold the TLS handshake. Runtime initialization is genuinely
    // waiting on network I/O while independent MCP calls remain responsive.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let blocked = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    let (first, first_child) = start(&directory.path().join("first"), &database, port).await;
    let (second, second_child) = start(&directory.path().join("second"), &database, port).await;
    assert_eq!(first.list_all_tools().await.unwrap().len(), 60);
    let request_id = RequestId::new().to_string();
    let args = json!({"device_code":"123456789","source":source,"destination":if cfg!(windows){"C:\\fixture\\remote.bin"}else{"/tmp/fixture-remote.bin"},"request_id":request_id});
    let submitted = result(call(&first, "pab_upload_file", args.clone()).await);
    assert_eq!(submitted["operation_ref"]["operation_id"], request_id);
    assert_eq!(submitted["complete"], false);
    assert_eq!(submitted["os_context_source"], "remembered_device");
    let reference = json!({"device_code":"123456789","operation_id":request_id});
    let query = result(call(&first, "pab_get_operation", reference.clone()).await);
    assert_eq!(query["operation_ref"], submitted["operation_ref"]);
    let page = result(call(&first, "pab_list_operations", json!({})).await);
    assert_eq!(page["operations"].as_array().unwrap().len(), 1);
    assert_eq!(
        result(call(&second, "pab_list_operations", json!({})).await)["operations"],
        json!([])
    );
    assert_eq!(
        call(&second, "pab_cancel_operation", reference.clone())
            .await
            .is_error,
        Some(true)
    );
    assert_eq!(
        call(&first, "pab_disconnect", json!({"device_code":"123456789"}))
            .await
            .is_error,
        Some(true)
    );
    let mut bad = args.clone();
    bad["wait_ms"] = json!(5001);
    assert_eq!(
        call(&first, "pab_upload_file", bad).await.is_error,
        Some(true)
    );
    let mut bad = args.clone();
    bad["unknown_field"] = json!(true);
    assert_eq!(
        call(&first, "pab_upload_file", bad).await.is_error,
        Some(true)
    );
    result(call(&first, "pab_cancel_operation", reference.clone()).await);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let query = result(call(&first, "pab_get_operation", reference.clone()).await);
            if query["complete"] == true {
                assert_eq!(query["operation"]["state"], "cancelled");
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let repeated = result(call(&first, "pab_upload_file", args.clone()).await);
    assert_eq!(repeated["operation"]["state"], "cancelled");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runtime_operations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        result(call(&first, "pab_disconnect", json!({"device_code":"123456789"})).await)["scope"],
        "current_mcp_session"
    );
    let mut first_pending = args.clone();
    first_pending["request_id"] = json!(RequestId::new());
    result(call(&first, "pab_upload_file", first_pending).await);
    let mut second_pending = args;
    let second_id = RequestId::new().to_string();
    second_pending["request_id"] = json!(second_id);
    result(call(&second, "pab_upload_file", second_pending).await);
    stop(first, first_child).await;
    let second_result = result(
        call(
            &second,
            "pab_get_operation",
            json!({"device_code":"123456789","operation_id":second_id}),
        )
        .await,
    );
    assert_eq!(
        second_result["complete"], false,
        "one MCP shutdown interrupted the other MCP"
    );
    stop(second, second_child).await;
    let cancelled: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM runtime_operations WHERE state = 'cancelled'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(cancelled, 3);
    blocked.abort();
    let _ = blocked.await;
}
