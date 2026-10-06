//! Opt-in remote regression for the built MCP executable, not installed-host acceptance.
//! Set PAB_TEST_DEVICE_CODE, PAB_TEST_PATH (an existing target file), and
//! PAB_DATA_DIR to the user's shared credential store,
//! plus PAB_CONTROL_URL/PAB_RELAY_URLS for the test server.
use serde_json::{Value, json};
use std::time::Duration;
mod support;
use support::{Client, stop};
async fn start() -> (Client, tokio::process::Child) {
    support::start(tokio::process::Command::new(env!("CARGO_BIN_EXE_pab-mcp"))).await
}

async fn call(client: &Client, name: &str, args: Value) -> Value {
    let response = support::call(client, name, args, Duration::from_secs(45)).await;
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
