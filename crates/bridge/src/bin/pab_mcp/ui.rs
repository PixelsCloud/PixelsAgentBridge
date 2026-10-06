use pab_protocol::{DesktopQuery, RequestId, SystemQuery, UiRequest};
use serde_json::{Value, json};
pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "pab_ui_query" | "pab_ui_get" | "pab_ui_action" | "pab_ui_wait"
    )
}
fn object(properties: Value, required: Vec<&str>) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn text(max: usize) -> Value {
    json!({"type":"string","maxLength":max})
}
fn integer(min: u32, max: u32) -> Value {
    json!({"type":"integer","minimum":min,"maximum":max})
}
fn variant(kind: &str, extra: Option<(&str, Value)>) -> Value {
    let mut props = json!({"type":{"type":"string","const":kind}});
    let mut required = vec!["type"];
    if let Some((key, value)) = extra {
        props[key] = value;
        required.push(key);
    }
    object(props, required)
}
pub fn tools() -> Vec<Value> {
    let reference = json!({"type":"string","format":"uuid"});
    let boolean = json!({"type":"boolean"});
    let check = json!({"type":"string","enum":["on","off","mixed"]});
    let scope = json!({"oneOf":[variant("window",Some(("window_ref",reference.clone()))),variant("element",Some(("element_ref",reference.clone())))]});
    let selector = object(
        json!({"role":{"type":"string","enum":["window","button","text_field","text","check_box","radio_button","list","list_item","combo_box","menu","menu_item","group","tab","tab_item","tree","tree_item","unknown"]},"name":text(512),"name_contains":text(512),"identifier":text(512)}),
        vec![],
    );
    let action = json!({"oneOf":[variant("invoke",None),variant("set_value",Some(("value",text(4096)))),variant("set_checked",Some(("checked",boolean.clone()))),variant("select",None),variant("expand",None),variant("collapse",None),variant("focus",None)]});
    let condition = json!({"oneOf":[variant("exists",None),variant("absent",None),variant("enabled",Some(("value",boolean.clone()))),variant("value_equals",Some(("value",text(4096)))),variant("checked",Some(("value",check.clone()))),variant("selected",Some(("value",boolean.clone())))]});
    ["pab_ui_query","pab_ui_get","pab_ui_action","pab_ui_wait"].into_iter().map(|name|{
        let mut props=json!({"device_code":{"type":"string","pattern":"^[0-9]{9}$"},"request_id":reference});
        let mut required=vec!["device_code"];
        let description=match name {
            "pab_ui_query"=>{
                props["scope"]=scope.clone();props["selector"]=selector.clone();required.push("scope");
                props["limits"]=object(json!({"limit":integer(1,500),"max_depth":integer(1,12),"max_visited":integer(1,2000),"timeout_ms":integer(100,10000)}),vec![]);
                "Query accessible controls within an exact window_ref from pab_list_windows or existing element_ref subtree. Defaults: 100 results, depth 6, 2000 visited, 3000 ms; 32 KiB reply. Exact name/identifier or explicit name_contains, no global desktop search or live-tree offset pagination. Returns all bounded matches; duplicates are not a unique target. Narrow scope on truncation. Values are omitted. References belong to this MCP connection/helper; query again after reconnect, worker failure or stale result."
            },
            "pab_ui_get"=>{props["element_ref"]=reference.clone();props["include_value"]=boolean.clone();required.push("element_ref");
                "Read current properties/capabilities for one existing element_ref. include_value defaults false; secure/password values are never read. Null or field_errors means unknown, not false. Windows bounds use physical pixels; macOS bounds use points, not screenshot pixels."},
            "pab_ui_action"=>{props["element_ref"]=reference.clone();props["action"]=action.clone();props["timeout_ms"]=integer(100,10000);
                props["expected"]=object(json!({"enabled":boolean,"read_only":boolean,"checked":check,"selected":boolean,"value":text(4096)}),vec![]);
                required.extend(["element_ref","action"]);
                "Perform one supported native control action on exact element_ref. invoke is the provider action, set_value replaces ordinary text, set_checked requests a desired state. No coordinate/clipboard fallback; secure fields rejected. expected preconditions are checked before dispatch, not an atomic UI transaction. Inspect action_dispatched, outcome and verification: native acceptance is not business success. Worker timeout/crash can mean unconfirmed effects. Never replay an uncertain action with a new ID. Windows UIA/macOS AX only; controls must expose corresponding native capabilities."},
            _=>{props["scope"]=scope.clone();props["selector"]=selector.clone();props["condition"]=condition.clone();props["timeout_ms"]=integer(100,30000);props["poll_ms"]=integer(100,1000);required.extend(["scope","condition"]);
                "Wait for a condition, default 5000 ms with 250 ms samples. Releases input queue between samples. Window selectors may discover matches; element scope never retargets. State conditions require one unique match; absent requires a complete successful query, not truncation/failure. Returns matched or timed_out; cancellation is cooperative. Cannot wait across top-level window changes: re-list windows."},
        };
        let mutation=name=="pab_ui_action";
        json!({"name":name,"description":format!("{description} Requires Executor system v9 and desktop helper v4. Returns operation_ref after about 250 ms while running. Preserve request_id; same ID/parameters deduplicates, changed parameters conflict. Use pab_get_operation and pab_cancel_operation; already dispatched effects are not rolled back. No provider-independent atomic snapshot or exclusive desktop lock."),"inputSchema":object(props,required),"annotations":{"readOnlyHint":!mutation,"destructiveHint":mutation,"idempotentHint":!mutation,"openWorldHint":false}})
    }).collect()
}
pub fn parse(name: &str, args: &Value) -> Result<(RequestId, SystemQuery), String> {
    if !handles(name) {
        return Err("unknown UI tool".into());
    }
    let mut fields = args
        .as_object()
        .cloned()
        .ok_or("arguments must be an object")?;
    fields.remove("device_code");
    let id = fields
        .remove("request_id")
        .map(serde_json::from_value::<RequestId>)
        .transpose()
        .map_err(|_| "invalid request_id")?
        .unwrap_or_default();
    if fields.contains_key("operation") {
        return Err("operation is determined by the tool name".into());
    }
    fields.insert(
        "operation".into(),
        json!(name.strip_prefix("pab_ui_").unwrap()),
    );
    let query: UiRequest =
        serde_json::from_value(Value::Object(fields)).map_err(|_| "invalid UI arguments")?;
    query.validate().map_err(str::to_owned)?;
    Ok((
        id,
        SystemQuery::Desktop {
            query: DesktopQuery::Ui { query },
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schemas_and_parser_reject_native_handles_unbounded_requests_and_injected_context() {
        let reference = RequestId::new().to_string();
        let good =
            json!({"device_code":"123456789","scope":{"type":"window","window_ref":reference}});
        assert!(super::super::mcp_catalog::validate_arguments("pab_ui_query", &good).is_ok());
        for extra in [
            json!({"ui_context":{"connection_id":reference}}),
            json!({"limits":{"limit":501}}),
            json!({"scope":{"type":"desktop"}}),
            json!({"scope":{"type":"window","window_ref":"123"}}),
        ] {
            let mut args = good.clone();
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(super::super::mcp_catalog::validate_arguments("pab_ui_query", &args).is_err());
        }
        let action = json!({"device_code":"123456789","element_ref":reference,"action":{"type":"set_value","value":"中文🙂"}});
        let (_, q) = parse("pab_ui_action", &action).unwrap();
        assert!(q.is_mutation());
        assert_eq!(q.required_version(), 9);
        assert!(
            !serde_json::to_string(&q.persistence_form())
                .unwrap()
                .contains("中文")
        );
        for tool in tools() {
            assert_eq!(
                tool["annotations"]["readOnlyHint"],
                tool["name"] != "pab_ui_action"
            );
        }
    }
}
