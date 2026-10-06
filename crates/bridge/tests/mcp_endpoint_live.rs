//! Opt-in remote regression for the built MCP executable, not installed-host acceptance.
//! Set PAB_TEST_DEVICE_CODE, PAB_TEST_PATH (an existing target file), and
//! PAB_DATA_DIR to the user's shared credential store,
//! plus PAB_CONTROL_URL/PAB_RELAY_URLS for the test server.
use serde_json::{Value, json};
use std::time::Duration;
mod support;
use support::{Client, stop};

#[tokio::test]
#[ignore = "requires an explicitly prepared foreground click fixture and result file"]
async fn monitor_click_reaches_owned_application_fixture() {
    let code = std::env::var("PAB_TEST_DEVICE_CODE").expect("device");
    let x: u32 = std::env::var("PAB_TEST_CLICK_X")
        .expect("fixture x")
        .parse()
        .unwrap();
    let y: u32 = std::env::var("PAB_TEST_CLICK_Y")
        .expect("fixture y")
        .parse()
        .unwrap();
    let path = std::env::var("PAB_TEST_CLICK_PATH").expect("fixture result path");
    let (client, child) = start().await;
    let connected = call(&client, "pab_connect", json!({"device_code":code})).await;
    assert_eq!(connected["connected"], true);
    let before = support::call(
        &client,
        "pab_file_read",
        json!({"device_code":code,"path":path}),
        Duration::from_secs(10),
    )
    .await;
    let before = before
        .structured_content
        .expect("fixture precondition result");
    assert_eq!(
        before["result"]["error"]["code"], "not_found",
        "result file must not already exist; no click sent"
    );
    let monitors = call(&client, "pab_list_monitors", json!({"device_code":code})).await;
    let target = monitors["result"]["data"]["snapshot"]["monitors"][0]["input_target"].clone();
    let id = pab_protocol::RequestId::new().to_string();
    let mut result = call(
        &client,
        "pab_desktop_input",
        json!({"device_code":code,"request_id":id,
        "monitor_input":{"target":target,"action":{"type":"click","x":x,"y":y,"button":"left"}}}),
    )
    .await;
    for _ in 0..20 {
        if result["result"]["state"] != "running" {
            break;
        }
        result = call(
            &client,
            "pab_get_operation",
            json!({"device_code":code,"operation_id":id,"wait_until":"complete","wait_ms":1000}),
        )
        .await;
    }
    assert_eq!(
        result["result"]["state"], "completed",
        "inspect original operation {id}"
    );
    // Read-only observation: the click is never repeated if application delivery is delayed.
    let mut observed = false;
    for _ in 0..20 {
        let raw = support::call(
            &client,
            "pab_file_read",
            json!({"device_code":code,"path":path}),
            Duration::from_secs(10),
        )
        .await;
        if raw
            .structured_content
            .as_ref()
            .and_then(|d| d["text"].as_str())
            == Some("clicked")
        {
            observed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    stop(client, child).await;
    assert!(
        observed,
        "application marker missing; do not replay click {id}"
    );
}
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
#[ignore = "requires a live device; changes only this test's local task snapshot to simulate a stopped observer"]
async fn status_queries_reconcile_stale_accepted_command_without_resubmitting() {
    let code = std::env::var("PAB_TEST_DEVICE_CODE").unwrap();
    let root = std::path::PathBuf::from(std::env::var_os("PAB_DATA_DIR").unwrap());
    let database = std::env::var_os("PAB_BRIDGE_DATABASE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("bridge.sqlite3"));
    let (client, child) = start().await;
    let target = call(&client, "pab_connect", json!({"device_code":code})).await;
    assert_eq!(target["connected"], true);
    let windows = target["target"]["execution"]["os_family"] == "windows";
    let id = pab_protocol::RequestId::new().to_string();
    let command = if windows {
        json!({"program":"cmd.exe","args":["/d","/c","echo PAB_status_fixture"]})
    } else {
        json!({"program":"/bin/echo","args":["PAB_status_fixture"]})
    };
    let mut args = command;
    args["device_code"] = json!(code);
    args["request_id"] = json!(id);
    args["wait_ms"] = json!(1000);
    let accepted = call(&client, "pab_run_command", args).await;
    let task_id = accepted["operation_ref"]["task_id"].clone();
    let finished = call(
        &client,
        "pab_get_operation",
        json!({"device_code":code,"operation_id":id,"wait_ms":10000,"wait_until":"complete"}),
    )
    .await;
    assert_eq!(finished["snapshot"]["state"], "succeeded");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(database))
        .await
        .unwrap();
    let original: String =
        sqlx::query_scalar("SELECT snapshot_json FROM runtime_tasks WHERE request_id = ?")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut stale: Value = serde_json::from_str(&original).unwrap();
    stale["state"] = json!("running");
    stale["completion"] = Value::Null;
    stale["finished_at_unix_ms"] = Value::Null;
    for name in ["pab_get_operation", "pab_get_task"] {
        sqlx::query("UPDATE runtime_tasks SET snapshot_json = ? WHERE request_id = ?")
            .bind(serde_json::to_string(&stale).unwrap())
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();
        let mut lookup = json!({"device_code":code,"wait_ms":10000,"wait_until":"complete"});
        if name == "pab_get_operation" {
            lookup["operation_id"] = json!(id);
        } else {
            lookup["task_id"] = task_id.clone();
        }
        let response = support::call(&client, name, lookup, Duration::from_secs(15)).await;
        // Restore our row even on assertion failure, never leave test corruption behind.
        sqlx::query("UPDATE runtime_tasks SET snapshot_json = ? WHERE request_id = ?")
            .bind(&original)
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();
        assert_ne!(response.is_error, Some(true), "{response:?}");
        let result = response.structured_content.unwrap();
        assert_eq!(
            result["snapshot"]["state"], "succeeded",
            "{name} kept a stale snapshot"
        );
        assert_eq!(result["snapshot"]["task_ref"]["task_id"], task_id);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    pool.close().await;
    stop(client, child).await;
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

#[tokio::test]
#[ignore = "requires an upgraded authorized desktop; moves pointer only, never clicks user content"]
async fn monitor_move_checks_geometry_session_and_deduplicates() {
    let code = std::env::var("PAB_TEST_DEVICE_CODE").expect("PAB_TEST_DEVICE_CODE");
    std::env::var_os("PAB_DATA_DIR").expect("explicit shared credential store required");
    let (client, child) = start().await;
    let mut connected = false;
    for _ in 0..24 {
        let state = call(&client, "pab_connect", json!({"device_code":code})).await;
        if state["connected"] == true {
            connected = true;
            break;
        }
    }
    assert!(
        connected,
        "connection remains pending; no pointer action sent"
    );
    let monitors = call(&client, "pab_list_monitors", json!({"device_code":code})).await;
    let target = monitors
        .pointer("/result/data/snapshot/monitors/0/input_target")
        .expect("upgrade Executor/helper to system v8/helper v3")
        .clone();
    let id = pab_protocol::RequestId::new().to_string();
    let args = json!({"device_code":code,"request_id":id,"monitor_input":{"target":target,"action":{"type":"move","x":100,"y":100}}});
    let mut result = call(&client, "pab_desktop_input", args.clone()).await;
    for _ in 0..20 {
        if result["result"]["state"] != "running" {
            break;
        }
        result = call(
            &client,
            "pab_get_operation",
            json!({"device_code":code,"operation_id":id,"wait_until":"complete","wait_ms":1000}),
        )
        .await;
    }
    assert_eq!(
        result["result"]["state"], "completed",
        "original operation {id} remains unconfirmed"
    );
    assert!(
        result["result"]["data"]["snapshot"]["verification"]
            .as_str()
            .unwrap()
            .contains("pointer observed")
    );
    let repeated = call(&client, "pab_desktop_input", args.clone()).await;
    assert_eq!(
        repeated["result"], result["result"],
        "same request ID must return the stored observation"
    );
    for field in ["x", "helper_instance", "id"] {
        let mut stale = args.clone();
        stale["request_id"] = json!(pab_protocol::RequestId::new());
        let target = &mut stale["monitor_input"]["target"];
        target[field] = match field {
            "x" => json!(target["x"].as_i64().unwrap() + 1),
            "id" => json!(u32::MAX),
            _ => json!(pab_protocol::RequestId::new()),
        };
        let raw = support::call(&client, "pab_desktop_input", stale, Duration::from_secs(15)).await;
        let mut rejected = raw.structured_content.unwrap();
        for _ in 0..20 {
            if rejected["result"]["state"] != "running" {
                break;
            }
            let reference = rejected["operation_ref"].clone();
            rejected = support::call(&client, "pab_get_operation", json!({"device_code":code,"operation_id":reference["operation_id"],"wait_until":"complete","wait_ms":1000}), Duration::from_secs(15))
                .await.structured_content.unwrap();
        }
        assert_eq!(rejected["result"]["state"], "failed", "{rejected}");
        assert_eq!(
            rejected["result"]["data"]["snapshot"]["action_started"],
            false
        );
    }
    stop(client, child).await;
}
