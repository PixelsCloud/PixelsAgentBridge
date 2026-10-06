//! Real stdio subprocess regression, using the MCP SDK and isolated storage.
//! This is transport testing, not a replacement for installed-host acceptance.
use pab_bridge::BridgeLocalStore;
use pab_protocol::{DeviceId, DeviceRef, RequestId, TenantId};
use rmcp::model::{CallToolRequestParams, CallToolResult};
mod support;
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use support::{Client, stop};

async fn start(root: &Path, database: &Path, port: u16) -> (Client, tokio::process::Child) {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_pab-mcp"));
    command
        .env("PAB_DATA_DIR", root)
        .env("PAB_BRIDGE_DATABASE", database)
        .env("PAB_CONTROL_URL", format!("wss://127.0.0.1:{port}"))
        .env("PAB_RELAY_URLS", "https://127.0.0.1:1");
    support::start(command).await
}
async fn call(client: &Client, name: &str, args: Value) -> CallToolResult {
    support::call(client, name, args, Duration::from_secs(2)).await
}

#[tokio::test]
async fn registered_catalog_has_coverage_and_exports_real_schemas() {
    let directory = tempfile::tempdir().unwrap();
    let (client, child) = start(
        directory.path(),
        &directory.path().join("catalog.sqlite3"),
        1,
    )
    .await;
    let listed = client.list_all_tools().await.unwrap();
    let coverage: Value =
        serde_json::from_str(include_str!("../../../acceptance/tool-coverage.json")).unwrap();
    let expected: std::collections::BTreeSet<_> = coverage["tools"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let actual: std::collections::BTreeSet<_> =
        listed.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(
        actual, expected,
        "register new tools in the acceptance coverage map"
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for case in coverage["tools"].as_object().unwrap().values() {
        for source in case["existing_test_sources"].as_array().unwrap() {
            assert!(
                root.join(source.as_str().unwrap()).is_file(),
                "missing test source {source}"
            );
        }
    }
    if let Some(path) = std::env::var_os("PAB_TEST_CATALOG_PATH") {
        std::fs::write(path, serde_json::to_vec_pretty(&listed).unwrap()).unwrap();
    }
    stop(client, child).await;
}

fn result(response: CallToolResult) -> Value {
    assert_ne!(response.is_error, Some(true), "{:?}", response.content);
    response
        .structured_content
        .expect("missing structured result")
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
    assert_eq!(first.list_all_tools().await.unwrap().len(), 64);
    let request_id = RequestId::new().to_string();
    let args = json!({"device_code":"123456789","source":source,"destination":if cfg!(windows){"C:\\fixture\\remote.bin"}else{"/tmp/fixture-remote.bin"},"request_id":request_id});
    let submitted = result(call(&first, "pab_upload_file", args.clone()).await);
    assert_eq!(submitted["operation_ref"]["operation_id"], request_id);
    assert_eq!(submitted["complete"], false);
    assert_eq!(submitted["os_context_source"], "remembered_device");
    let reference = json!({"device_code":"123456789","operation_id":request_id});
    let query = result(call(&first, "pab_get_operation", reference.clone()).await);
    assert_eq!(query["operation_ref"], submitted["operation_ref"]);
    let mut waiting = reference.clone();
    waiting["after_revision"] = query["revision"].clone();
    waiting["wait_ms"] = json!(10);
    waiting["wait_until"] = json!("complete");
    let pending = result(call(&first, "pab_get_operation", waiting).await);
    assert_eq!(pending["wait_expired"], true);
    assert_eq!(pending["complete"], false);
    assert_eq!(pending["operation_ref"], submitted["operation_ref"]);
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

#[tokio::test]
async fn tool_groups_are_fixed_at_start_and_disabled_calls_never_initialize_runtime() {
    use pab_bridge::mcp_tool_settings::{McpToolSettings, ToolGroup};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let database = root.join("bridge.sqlite3");
    let path = root.join("mcp-tools.json");
    let settings = McpToolSettings {
        version: 1,
        enabled_groups: vec![ToolGroup::Core],
    };
    settings.save(&path).unwrap();
    let (first, first_child) = start(root, &database, 1).await;
    let listed = first.list_all_tools().await.unwrap();
    assert_eq!(listed.len(), ToolGroup::Core.tools().len());
    for name in [
        "pab_git_status",
        "pab_list_containers",
        "pab_capture_screenshot",
        "pab_upload_file",
        "pab_system_info",
    ] {
        let response = call(&first, name, json!({"device_code":"123456789"})).await;
        assert_eq!(response.is_error, Some(true));
        assert!(format!("{:?}", response.content).contains("unavailable in this MCP process"));
    }
    assert!(
        !database.exists(),
        "disabled calls must not initialize the runtime/store"
    );
    McpToolSettings::default().save(&path).unwrap();
    assert_eq!(first.list_all_tools().await.unwrap().len(), 14);
    let (second, second_child) = start(root, &database, 1).await;
    assert_eq!(second.list_all_tools().await.unwrap().len(), 64);
    stop(second, second_child).await;
    stop(first, first_child).await;
}

#[tokio::test]
async fn invalid_tool_settings_fail_startup_without_exposing_a_fallback_catalog() {
    for (content, expected) in [
        (
            r#"{"version":2,"enabledGroups":["core"]}"#,
            "Unsupported MCP tool settings version",
        ),
        (
            r#"{"version":1,"enabledGroups":["core"],"invalid":true}"#,
            "unknown field",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("mcp-tools.json"), content).unwrap();
        let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_pab-mcp"))
            .env("PAB_DATA_DIR", directory.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            output.stdout.is_empty(),
            "Invalid preferences must not expose tools"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains(expected));
        assert!(!directory.path().join("bridge.sqlite3").exists());
    }
}

async fn offline_connect(client: &Client) {
    let response = tokio::time::timeout(
        Duration::from_secs(20),
        client.call_tool(
            CallToolRequestParams::new("pab_connect").with_arguments(
                json!({"device_code":"123456789","wait_ms":30000})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.is_error, Some(true));
}

#[tokio::test]
async fn short_connection_wait_keeps_stdio_responsive_and_validation_is_structured() {
    let directory = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (client, child) = start(
        directory.path(),
        &directory.path().join("bridge.sqlite3"),
        listener.local_addr().unwrap().port(),
    )
    .await;
    for _ in 0..2 {
        let response = result(
            call(
                &client,
                "pab_connect",
                json!({"device_code":"123456789","wait_ms":10}),
            )
            .await,
        );
        assert_eq!(response["state"], "connecting");
        assert_eq!(response["wait_expired"], true);
        assert_eq!(response["connection_ref"]["device_code"], "123456789");
        let _ = result(call(&client, "pab_list_devices", json!({})).await);
    }
    let disconnected = result(
        call(
            &client,
            "pab_disconnect",
            json!({"device_code":"123456789"}),
        )
        .await,
    );
    assert_eq!(disconnected["disconnected"], true);
    let error = call(
        &client,
        "pab_run_command",
        json!({"device_code":"123456789","program":"echo","args":[],"timeout_ms":0}),
    )
    .await;
    assert_eq!(error.is_error, Some(true));
    assert_eq!(
        error.structured_content.unwrap()["error"]["code"],
        "invalid_arguments"
    );
    let batch_tool = client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.name == "pab_desktop_input")
        .unwrap();
    assert!(
        batch_tool.input_schema["properties"]
            .get("actions")
            .is_some()
    );
    let invalid = call(
        &client,
        "pab_desktop_input",
        json!({
            "device_code":"123456789", "window_ref":RequestId::new(),
            "actions":[{"type":"focus"},{"type":"type_text","text":""}]
        }),
    )
    .await;
    assert_eq!(invalid.is_error, Some(true));
    let invalid = invalid.structured_content.unwrap();
    assert_eq!(invalid["error"]["phase"], "validation");
    assert!(
        invalid["operation_ref"]["operation_id"]
            .as_str()
            .unwrap()
            .parse::<RequestId>()
            .is_ok()
    );
    stop(client, child).await;
}

#[tokio::test]
async fn same_user_mcp_processes_keep_distinct_keys_across_retries_and_crash() {
    use pab_agent_core::{load_or_create_endpoint_secret, read_endpoint_secret};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let database = root.join("bridge.sqlite3");
    let desktop_path = root.join("guest-endpoint.key");
    let desktop = load_or_create_endpoint_secret(&desktop_path)
        .unwrap()
        .public();
    // Reserve an unreachable local control port without depending on external DNS.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (first, mut first_child) = start(root, &database, port).await;
    let (second, second_child) = start(root, &database, port).await;
    tokio::join!(offline_connect(&first), offline_connect(&second));
    let key0 = root.join("mcp-endpoints/guest-0.key");
    let key1 = root.join("mcp-endpoints/guest-1.key");
    let identities = [
        read_endpoint_secret(&key0).unwrap().public(),
        read_endpoint_secret(&key1).unwrap().public(),
    ];
    assert_ne!(identities[0], identities[1]);
    assert!(identities.iter().all(|key| *key != desktop));
    // A forced termination cannot leave a slot permanently occupied.
    first_child.kill().await.unwrap();
    first_child.wait().await.unwrap();
    let _ = first.cancel().await;
    let (third, third_child) = start(root, &database, port).await;
    tokio::join!(offline_connect(&second), offline_connect(&third));
    assert!(!root.join("mcp-endpoints/guest-2.key").exists());
    assert_eq!(read_endpoint_secret(&key0).unwrap().public(), identities[0]);
    assert_eq!(read_endpoint_secret(&key1).unwrap().public(), identities[1]);
    assert_eq!(
        read_endpoint_secret(&desktop_path).unwrap().public(),
        desktop
    );
    stop(second, second_child).await;
    stop(third, third_child).await;
}
