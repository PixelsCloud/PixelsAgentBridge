//! Opt-in remote regression for the built MCP executable, not installed-host acceptance.
//! Set PAB_TEST_DEVICE_CODE, PAB_TEST_PATH (an existing target file), and
//! PAB_DATA_DIR to the user's shared credential store,
//! plus PAB_CONTROL_URL/PAB_RELAY_URLS for the test server.
use std::{process::Stdio, time::Duration};

use rmcp::{RoleClient, ServiceExt, model::CallToolRequestParams, service::RunningService};
use serde_json::{Value, json};

type Client = RunningService<RoleClient, ()>;

async fn start() -> (Client, tokio::process::Child) {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_pab-mcp"))
        .env("PAB_MCP_GUEST", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    let client = tokio::time::timeout(Duration::from_secs(10), ().serve((stdout, stdin)))
        .await
        .unwrap()
        .unwrap();
    (client, child)
}

async fn call(client: &Client, name: &str, args: Value) -> Value {
    let response = tokio::time::timeout(
        Duration::from_secs(45),
        client.call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        ),
    )
    .await
    .expect("remote tool timed out")
    .unwrap();
    assert_ne!(
        response.is_error,
        Some(true),
        "{}: {:?}",
        name,
        response.content
    );
    let data = response.structured_content.unwrap();
    assert_ne!(data.pointer("/result/state"), Some(&json!("failed")));
    data
}

async fn query(client: &Client, code: &str) {
    // Exercise concurrent transport without contending for the single OS
    // system-info collector, which deliberately returns executor_busy.
    let path = std::env::var("PAB_TEST_PATH").expect("PAB_TEST_PATH");
    let info = call(
        client,
        "pab_file_stat",
        json!({"device_code":code,"path":path}),
    )
    .await;
    assert_eq!(info["result"]["state"], "completed");
    assert!(info["result"]["metadata"]["kind"].is_string());
}

async fn stop(client: Client, mut child: tokio::process::Child) {
    client.cancel().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[tokio::test]
#[ignore = "requires an authorized live device and shared local credentials"]
async fn two_mcp_processes_connect_disconnect_and_restart_independently() {
    let code = std::env::var("PAB_TEST_DEVICE_CODE").expect("PAB_TEST_DEVICE_CODE");
    std::env::var_os("PAB_DATA_DIR").expect("PAB_DATA_DIR must explicitly select the shared store");
    let ((first, mut first_child), (second, second_child)) = tokio::join!(start(), start());
    tokio::join!(
        call(&first, "pab_connect", json!({"device_code":code})),
        call(&second, "pab_connect", json!({"device_code":code})),
    );
    tokio::join!(query(&first, &code), query(&second, &code));
    call(&first, "pab_disconnect", json!({"device_code":code})).await;
    query(&second, &code).await;
    call(&first, "pab_connect", json!({"device_code":code})).await;
    tokio::join!(query(&first, &code), query(&second, &code));
    // Keep a live peer connected while another MCP crashes and its slot is reused.
    first_child.kill().await.unwrap();
    first_child.wait().await.unwrap();
    let _ = first.cancel().await;
    let (third, third_child) = start().await;
    tokio::join!(
        call(&third, "pab_connect", json!({"device_code":code})),
        query(&second, &code),
    );
    tokio::join!(query(&third, &code), query(&second, &code));
    stop(third, third_child).await;
    query(&second, &code).await;
    stop(second, second_child).await;
}
