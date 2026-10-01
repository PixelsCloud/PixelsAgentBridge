use pab_bridge::BridgeRuntime;
use pab_protocol::{DesktopQuery, RequestId, SystemQuery, WindowControlAction};
use serde_json::{Value, json};
pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "pab_list_monitors"
            | "pab_list_windows"
            | "pab_focus_window"
            | "pab_window_control"
            | "pab_type_text"
    )
}
pub fn tools() -> Vec<Value> {
    ["pab_list_monitors","pab_focus_window","pab_window_control","pab_type_text"].into_iter().map(|name|{
        let mutation=name!="pab_list_monitors";
        let mut props=json!({"device_code":{"type":"string","pattern":"^[0-9]{9}$"},"request_id":{"type":"string","format":"uuid"}});
        let mut required=vec!["device_code"];
        if mutation {props["window_ref"]=json!({"type":"string","format":"uuid"});required.push("window_ref");}
        if name=="pab_window_control" {props["control"]=json!({"type":"string","enum":["minimize","maximize","restore","close"]});required.push("control");}
        if name=="pab_type_text" {props["text"]=json!({"type":"string","minLength":1,"maxLength":4096});required.push("text");}
        let description=match name {"pab_list_monitors"=>"List a bounded snapshot of display IDs, names, origins, dimensions, primary state, scale and rotation via xcap; IDs can change after hotplug.","pab_focus_window"=>"Focus the exact opaque window_ref from pab_list_windows; verifies foreground, respects OS policy and does not bypass foreground restrictions.","pab_window_control"=>"Minimize/maximize/restore or request normal close of a referenced external window. Verify observed state; close may be refused or show a save dialog. Never kills the process.",_=>"Enter 1..4096 UTF-8 bytes using Enigo into an explicitly referenced FOREGROUND window; call pab_focus_window first. Input API acceptance is not proof of application content; focus/desktop changes can cause partial input. No clipboard replacement; raw text not stored in audit records."};
        json!({"name":name,"description":format!("{description} Requires upgraded Executor (system capability v4) and desktop helper. Windows/X11; Wayland unsupported. Same request_id is never replayed; preserve operation_ref and query pab_get_operation when running/unconfirmed. Mutations return running after about 250 ms if still active; they cannot be cancelled or undone."),"inputSchema":{"type":"object","properties":props,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":!mutation,"destructiveHint":mutation,"idempotentHint":!mutation,"openWorldHint":false}})
    }).collect()
}
pub fn parse(name: &str, args: &Value) -> Result<(RequestId, SystemQuery), String> {
    let id = args
        .get("request_id")
        .map(|v| {
            v.as_str()
                .ok_or("request_id must be a string")?
                .parse()
                .map_err(|_| "invalid request_id")
        })
        .transpose()?
        .unwrap_or_default();
    let reference = || {
        args.get("window_ref")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or("window_ref required")
    };
    let query = match name {
        "pab_list_monitors" => DesktopQuery::Monitors {},
        "pab_list_windows" => DesktopQuery::Windows {},
        "pab_focus_window" => DesktopQuery::Focus {
            window_ref: reference()?,
        },
        "pab_window_control" => DesktopQuery::Control {
            window_ref: reference()?,
            control: serde_json::from_value::<WindowControlAction>(
                args.get("control").cloned().ok_or("control required")?,
            )
            .map_err(|e| e.to_string())?,
        },
        "pab_type_text" => DesktopQuery::TypeText {
            window_ref: reference()?,
            text: args
                .get("text")
                .and_then(Value::as_str)
                .ok_or("text required")?
                .to_owned(),
        },
        _ => return Err("unknown desktop tool".into()),
    };
    query.validate().map_err(str::to_owned)?;
    Ok((id, SystemQuery::Desktop { query }))
}
pub async fn call(runtime: &BridgeRuntime, name: &str, args: &Value) -> Result<Value, String> {
    let (id, query) = parse(name, args)?;
    let device = super::mcp_tools::resolve_target(runtime, args).await?;
    let target = runtime
        .current_environment(device)
        .await
        .map_err(|e| e.to_string())?;
    let result = runtime
        .system_query(device, id, query)
        .await
        .map_err(|e| e.to_string())?;
    Ok(
        json!({"device_ref":device,"operation_ref":{"device_code":args["device_code"],"operation_id":id,"kind":result.kind},"result":result,"os_reminder":target.compact_reminder()}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_tools_validate_references_control_and_utf8_budget_before_dispatch() {
        let reference = RequestId::new().to_string();
        let args = json!({"device_code":"123456789","window_ref":reference,"text":"中文🙂"});
        assert!(parse("pab_type_text", &args).is_ok());
        for text in ["".to_owned(), "中".repeat(1366), "nul\0".to_owned()] {
            assert!(
                parse(
                    "pab_type_text",
                    &json!({"window_ref":reference,"text":text})
                )
                .is_err()
            );
        }
        assert!(
            parse(
                "pab_window_control",
                &json!({"window_ref":reference,"control":"kill"})
            )
            .is_err()
        );
        assert!(parse("pab_focus_window", &json!({"window_ref":"12345"})).is_err());
        for tool in tools() {
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        }
    }
}
