//! Explicitly enabled real stdio/QUIC/helper integration. This is independent
//! client evidence, not proof that the current AI host reloaded its tools.
mod support;
use serde_json::{Value, json};
use std::time::Duration;
use support::{Client, stop};

async fn invoke(c: &Client, code: &str, name: &str, mut args: Value) -> Value {
    args["device_code"] = json!(code);
    let id = pab_protocol::RequestId::new();
    args["request_id"] = json!(id);
    // Emit only identifiers, never entered text or returned UI contents.
    eprintln!("UI acceptance {name} request={id}");
    let mut v = support::call(c, name, args, Duration::from_secs(20))
        .await
        .structured_content
        .expect("structured result");
    for _ in 0..40 {
        if v["result"]["state"] != "running" && v["result"]["state"] != "pending" {
            return v;
        }
        v = support::call(
            c,
            "pab_get_operation",
            json!({"device_code":code,"operation_id":id,"wait_until":"complete","wait_ms":1000}),
            Duration::from_secs(15),
        )
        .await
        .structured_content
        .unwrap();
    }
    panic!("Unconfirmed UI operation {id}; do not replay");
}
fn ui(v: &Value) -> &Value {
    assert_eq!(
        v["result"]["state"], "completed",
        "UI request failed; inspect emitted ID"
    );
    &v["result"]["data"]["snapshot"]["ui"]
}
async fn start(code: &str) -> (Client, tokio::process::Child) {
    let binary = std::env::var_os("PAB_TEST_MCP_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_pab-mcp").into());
    let (client, child) = support::start(tokio::process::Command::new(binary)).await;
    for _ in 0..24 {
        let r = support::call(
            &client,
            "pab_connect",
            json!({"device_code":code}),
            Duration::from_secs(10),
        )
        .await;
        if r.structured_content
            .as_ref()
            .is_some_and(|v| v["connected"] == true)
        {
            return (client, child);
        }
    }
    panic!("connection pending; no actions dispatched");
}

#[tokio::test]
#[ignore = "requires upgraded target and a fresh owned ui_controls fixture with PAB_TEST_UI_RESULT"]
async fn controls_reach_application_and_connection_references_are_isolated() {
    let code = std::env::var("PAB_TEST_DEVICE_CODE").expect("device code");
    let path = std::env::var("PAB_TEST_UI_RESULT").expect("fresh fixture result path");
    std::env::var_os("PAB_DATA_DIR").expect("explicit credential store");
    let (client, child) = start(&code).await;
    let pre = support::call(
        &client,
        "pab_file_read",
        json!({"device_code":code,"path":path}),
        Duration::from_secs(10),
    )
    .await
    .structured_content
    .unwrap();
    assert_eq!(
        pre["result"]["error"]["code"], "not_found",
        "fixture has already been used"
    );
    let windows = invoke(&client, &code, "pab_list_windows", json!({})).await;
    let windows = windows["result"]["data"]["snapshot"]["windows"]
        .as_array()
        .expect("window list");
    let matching: Vec<_> = windows
        .iter()
        .filter(|w| w["title"] == "PAB UI acceptance fixture")
        .collect();
    assert_eq!(matching.len(), 1, "exactly one owned fixture must exist");
    let scope = json!({"type":"window","window_ref":matching[0]["window_ref"]});
    let tree = invoke(&client, &code, "pab_ui_query", json!({"scope":scope})).await;
    let tree = ui(&tree);
    assert_eq!(tree["truncated"], false);
    let elements = tree["elements"].as_array().unwrap();
    let reference = |name: &str| -> String {
        let targets: Vec<_> = elements.iter().filter(|e| e["name"] == name).collect();
        assert_eq!(
            targets.len(),
            1,
            "fixture control missing/ambiguous: {name}"
        );
        targets[0]["element_ref"].as_str().unwrap().into()
    };
    let edit = reference("Fixture input");
    let secret = invoke(
        &client,
        &code,
        "pab_ui_get",
        json!({"element_ref":reference("Fixture secure"),"include_value":true}),
    )
    .await;
    assert_eq!(ui(&secret)["elements"][0]["protected"], true);
    assert!(ui(&secret)["elements"][0]["value"].is_null());
    let (second, second_child) = start(&code).await;
    let rejected = invoke(&second, &code, "pab_ui_get", json!({"element_ref":edit})).await;
    assert_eq!(
        rejected["result"]["data"]["snapshot"]["ui"]["error_code"],
        "stale_element"
    );
    stop(second, second_child).await;
    for (name, action) in [
        (
            "Fixture input",
            json!({"type":"set_value","value":"PAB 中文🙂"}),
        ),
        (
            "Fixture option",
            json!({"type":"set_checked","checked":true}),
        ),
        ("Fixture radio", json!({"type":"select"})),
    ] {
        let result = invoke(
            &client,
            &code,
            "pab_ui_action",
            json!({"element_ref":reference(name),"action":action}),
        )
        .await;
        assert_eq!(ui(&result)["verification"], "matched");
    }
    let waited=invoke(&client,&code,"pab_ui_wait",json!({"scope":{"type":"element","element_ref":edit},"condition":{"type":"value_equals","value":"PAB 中文🙂"},"timeout_ms":2000})).await;
    assert_eq!(ui(&waited)["outcome"], "matched");
    let absent=invoke(&client,&code,"pab_ui_wait",json!({"scope":scope,"selector":{"name":"nonexistent acceptance"},"condition":{"type":"exists"},"timeout_ms":500,"poll_ms":100})).await;
    assert_eq!(ui(&absent)["outcome"], "timed_out");
    let id = pab_protocol::RequestId::new();
    let args = json!({"device_code":code,"request_id":id,"element_ref":reference("Apply fixture"),"action":{"type":"invoke"}});
    eprintln!("UI acceptance single invoke request={id}");
    let first = support::call(
        &client,
        "pab_ui_action",
        args.clone(),
        Duration::from_secs(20),
    )
    .await
    .structured_content
    .unwrap();
    let mut final_result = first;
    for _ in 0..20 {
        if final_result["result"]["state"] != "running" {
            break;
        }
        final_result = support::call(
            &client,
            "pab_get_operation",
            json!({"device_code":code,"operation_id":id,"wait_ms":1000,"wait_until":"complete"}),
            Duration::from_secs(15),
        )
        .await
        .structured_content
        .unwrap();
    }
    assert_eq!(ui(&final_result)["action_dispatched"], true);
    let replay = support::call(&client, "pab_ui_action", args, Duration::from_secs(10))
        .await
        .structured_content
        .unwrap();
    assert_eq!(replay["result"], final_result["result"]);
    let mut observed = false;
    for _ in 0..30 {
        let raw = support::call(
            &client,
            "pab_file_read",
            json!({"device_code":code,"path":path}),
            Duration::from_secs(10),
        )
        .await
        .structured_content
        .unwrap();
        if let Some(text) = raw["text"].as_str() {
            let actual: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
            assert_eq!(actual["clicks"], 1);
            assert_eq!(actual["value"], "PAB 中文🙂");
            assert!(actual["checked"] == true || actual["checked"] == 1);
            assert!(actual["radio"] == true || actual["radio"] == 1);
            observed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        observed,
        "actual fixture effect not observed; never replay the invoke"
    );
    stop(client, child).await;
}
