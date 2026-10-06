use pab_bridge::BridgeRuntime;
use pab_protocol::{DesktopQuery, RequestId, SystemQuery, WindowControlAction};
use serde_json::{Value, json};
pub fn handles(name: &str) -> bool {
    if super::mcp_ui::handles(name) {
        return true;
    }
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
    if super::mcp_ui::handles(name) {
        return super::mcp_ui::parse(name, args);
    }
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
        "pab_desktop_input" if args.get("monitor_input").is_some() => {
            if ["event", "actions", "window_ref", "timeout_ms"]
                .iter()
                .any(|key| args.get(key).is_some())
            {
                return Err(
                    "monitor_input cannot be combined with legacy event or window batch parameters"
                        .into(),
                );
            }
            DesktopQuery::MonitorInput {
                input: serde_json::from_value(args["monitor_input"].clone())
                    .map_err(|e| e.to_string())?,
            }
        }
        "pab_desktop_input" => {
            if args.get("event").is_some() {
                return Err("event and batch actions are mutually exclusive".into());
            }
            DesktopQuery::Batch {
                window_ref: reference()?,
                actions: serde_json::from_value(
                    args.get("actions").cloned().ok_or("actions required")?,
                )
                .map_err(|e| e.to_string())?,
                timeout_ms: match args.get("timeout_ms") {
                    None => 5000,
                    Some(v) => v
                        .as_u64()
                        .and_then(|v| u32::try_from(v).ok())
                        .ok_or("timeout_ms must be an integer")?,
                },
            }
        }
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

pub fn enhance_input_tool(tool: &mut Value) {
    tool["description"] = json!(
        "Send legacy event OR a window-bound ordered batch (actions), never both. Batch requires window_ref from pab_list_windows, Executor system v7 and helper v2. 1..32 actions: focus, control, type_text, key_chord, click, scroll, wait. Text total <=4096 UTF-8 bytes. Stops on first error, reports zero-based per-step completed/failed/unconfirmed/skipped results; no rollback or automatic replay. Use request_id for batch deduplication; query original operation_ref when running/unconfirmed. timeout_ms (default 5000, 100..10000) is checked between actions, not a hard OS-call deadline. Helper serializes the whole batch with other helper input, but cannot exclude physical input/other software. Keys/buttons are released on ordinary failures. Click coordinates are pixels relative to the current outer window rectangle, NOT screenshot image pixels. Click/scroll require the pointer to target this foreground window; text/keys also require foreground, so start with focus if needed. Input API acceptance does not verify application effects. Legacy event retains its existing behavior and does not accept batch parameters: mouse_move (x/y 0..65535), mouse_button (button left/right/middle, down), mouse_wheel (delta), key (virtual_key, down), secure_attention (Windows Ctrl+Alt+Delete)."
    );
    let p = &mut tool["inputSchema"]["properties"];
    let integer = |min: i64, max: i64| json!({"type":"integer","minimum":min,"maximum":max});
    let target = json!({"type":"object","additionalProperties":false,"required":["helper_instance","id","x","y","width","height","scale_percent","rotation_degrees","coordinate_space"],"properties":{
        "helper_instance":{"type":"string","format":"uuid"},"id":integer(0,u32::MAX.into()),
        "x":integer(i32::MIN.into(),i32::MAX.into()),"y":integer(i32::MIN.into(),i32::MAX.into()),
        "width":integer(1,65535),"height":integer(1,65535),"scale_percent":integer(1,800),
        "rotation_degrees":integer(0,360),"coordinate_space":{"type":"string","enum":["physical_pixels","logical_points"]}
    }});
    let monitor_action = |click: bool| {
        let mut props = json!({"type":{"type":"string","const":if click {"click"} else {"move"}},"x":integer(0,65535),"y":integer(0,65535)});
        let mut required = vec!["type", "x", "y"];
        if click {
            props["button"] = json!({"type":"string","enum":["left","right","middle"]});
            required.push("button");
        }
        json!({"type":"object","additionalProperties":false,"required":required,"properties":props})
    };
    p["monitor_input"] = json!({"type":"object","additionalProperties":false,"required":["target","action"],"properties":{
        "target":target,"action":{"oneOf":[monitor_action(false),monitor_action(true)]}
    }});
    p["window_ref"] = json!({"type":"string","format":"uuid"});
    p["request_id"] = json!({"type":"string","format":"uuid"});
    p["timeout_ms"] = json!({"type":"integer","minimum":100,"maximum":10000,"default":5000});
    let action = |kind: &str, extra: Value, required: Vec<&str>| {
        let mut properties = json!({"type":{"type":"string","const":kind}});
        for (k, v) in extra.as_object().unwrap() {
            properties[k] = v.clone();
        }
        let mut fields = vec!["type"];
        fields.extend(required);
        json!({"type":"object","properties":properties,"required":fields,"additionalProperties":false})
    };
    p["actions"] = json!({"type":"array","minItems":1,"maxItems":32,"items":{"oneOf":[
        action("focus",json!({}),vec![]),
        action("control",json!({"control":{"type":"string","enum":["minimize","maximize","restore","close"]}}),vec!["control"]),
        action("type_text",json!({"text":{"type":"string","minLength":1,"maxLength":4096}}),vec!["text"]),
        action("click",json!({"x":{"type":"integer","minimum":0,"maximum":65535},"y":{"type":"integer","minimum":0,"maximum":65535},"button":{"type":"string","enum":["left","right","middle"]}}),vec!["x","y","button"]),
        action("key_chord",json!({"modifiers":{"type":"array","maxItems":4,"uniqueItems":true,"items":{"type":"string","enum":["control","alt","shift","meta"]}},"key":{"type":"string","pattern":"^([a-z0-9]|enter|tab|escape|space|backspace|delete|left|right|up|down|home|end|page_up|page_down|f([1-9]|1[0-2]))$"}}),vec!["modifiers","key"]),
        action("scroll",json!({"axis":{"type":"string","enum":["horizontal","vertical"]},"amount":{"type":"integer","minimum":-100,"maximum":100,"not":{"const":0}}}),vec!["axis","amount"]),
        action("wait",json!({"ms":{"type":"integer","minimum":1,"maximum":2000}}),vec!["ms"])
    ]}});
    tool["inputSchema"]["required"] = json!(["device_code"]);
    tool["inputSchema"]["oneOf"] = json!([
        {"required":["event"],"not":{"anyOf":[{"required":["monitor_input"]},{"required":["actions"]},{"required":["window_ref"]},{"required":["timeout_ms"]},{"required":["request_id"]}]}},
        {"required":["actions","window_ref"],"not":{"anyOf":[{"required":["event"]},{"required":["monitor_input"]}]}},
        {"required":["monitor_input"],"not":{"anyOf":[{"required":["event"]},{"required":["actions"]},{"required":["window_ref"]},{"required":["timeout_ms"]}]}}
    ]);
    let original = tool["description"].as_str().unwrap().to_owned();
    tool["description"] = json!(format!(
        "{original} Alternatively use monitor_input with target copied unchanged from pab_list_monitors.input_target and action move/click. Requires Executor system v8/helper v3. x/y are integer logical units relative to that monitor; Windows/X11 native pixels = logical * scale_percent/100, macOS native points = logical. Never use screenshot pixels directly: map image coordinates via desktop_rect and subtract target origin, then convert native units to logical units. Monitor/session/scale changes fail instead of falling back. request_id deduplicates this entire move/click; unconfirmed actions must never be replayed blindly."
    ));
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
    fn monitor_input_schema_parser_and_version_are_strict() {
        let target = json!({"helper_instance":RequestId::new(),"id":2,"x":-1920,"y":0,"width":1920,"height":1080,"scale_percent":150,"rotation_degrees":0,"coordinate_space":"physical_pixels"});
        let valid = json!({"device_code":"123456789","monitor_input":{"target":target,"action":{"type":"click","x":100,"y":200,"button":"left"}}});
        assert!(super::super::mcp_catalog::validate_arguments("pab_desktop_input", &valid).is_ok());
        let (_, query) = parse("pab_desktop_input", &valid).unwrap();
        assert_eq!(query.required_version(), 8);
        assert_eq!(query.kind(), "monitor_input");
        if let SystemQuery::Desktop { query } = query {
            assert_eq!(query.required_helper_version(), 3);
        }
        for (key, value) in [
            ("event", json!({"type":"mouse_move","x":1,"y":1})),
            ("actions", json!([{"type":"focus"}])),
            ("window_ref", json!(RequestId::new())),
            ("timeout_ms", json!(5000)),
        ] {
            let mut bad = valid.clone();
            bad[key] = value;
            assert!(
                super::super::mcp_catalog::validate_arguments("pab_desktop_input", &bad).is_err()
            );
        }
        for x in [
            json!(-1),
            json!(1280),
            json!(1.5),
            json!(null),
            json!(u64::MAX),
        ] {
            let mut bad = valid.clone();
            bad["monitor_input"]["action"]["x"] = x;
            assert!(
                super::super::mcp_catalog::validate_arguments("pab_desktop_input", &bad).is_err()
            );
        }
    }
    #[test]
    fn input_batch_schema_and_parser_reject_mixed_or_incomplete_requests() {
        let reference = RequestId::new().to_string();
        let valid = json!({"device_code":"123456789","window_ref":reference,"actions":[{"type":"focus"},{"type":"key_chord","modifiers":["control"],"key":"a"},{"type":"type_text","text":"中文"}]});
        assert!(super::super::mcp_catalog::validate_arguments("pab_desktop_input", &valid).is_ok());
        let (_, query) = parse("pab_desktop_input", &valid).unwrap();
        assert_eq!(query.required_version(), 7);
        assert_eq!(query.kind(), "desktop_batch");
        let legacy =
            json!({"device_code":"123456789","event":{"type":"key","virtual_key":13,"down":true}});
        assert!(
            super::super::mcp_catalog::validate_arguments("pab_desktop_input", &legacy).is_ok()
        );
        for invalid in [
            json!({"device_code":"123456789"}),
            json!({"device_code":"123456789","actions":[]}),
            json!({"device_code":"123456789","window_ref":reference,"actions":[{"type":"focus","extra":true}]}),
            json!({"device_code":"123456789","window_ref":reference,"actions":[{"type":"scroll","axis":"vertical","amount":0}]}),
            json!({"device_code":"123456789","window_ref":reference,"actions":[{"type":"key_chord","modifiers":[],"key":"inject"}]}),
        ] {
            assert!(
                super::super::mcp_catalog::validate_arguments("pab_desktop_input", &invalid)
                    .is_err(),
                "{invalid}"
            );
        }
        let mut mixed = valid.clone();
        mixed["event"] = legacy["event"].clone();
        assert!(
            super::super::mcp_catalog::validate_arguments("pab_desktop_input", &mixed).is_err()
        );
        let mut bad_legacy = legacy;
        bad_legacy["request_id"] = json!(RequestId::new());
        assert!(
            super::super::mcp_catalog::validate_arguments("pab_desktop_input", &bad_legacy)
                .is_err()
        );
    }
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
