use base64::{Engine, engine::general_purpose::STANDARD};
use pab_bridge::BridgeRuntime;
use pab_protocol::MobileRequest;
use rmcp::model::ContentBlock;
use serde_json::{Value, json};

pub fn tools() -> Vec<Value> {
    let fields = json!({
        "element_ref":{"type":"string","format":"uuid"},
        "root_ref":{"type":["string","null"],"format":"uuid"},
        "text_contains":{"type":["string","null"],"maxLength":1024},
        "role":{"type":["string","null"],"maxLength":256},
        "max_nodes":{"type":"integer","minimum":1,"maximum":128},
        "max_depth":{"type":"integer","minimum":1,"maximum":32},
        "text":{"type":"string","maxLength":4096},
        "present":{"type":"boolean"},
        "timeout_ms":{"type":"integer","minimum":100,"maximum":20000},
        "interval_ms":{"type":"integer","minimum":100,"maximum":1000},
        "package_name":{"type":"string","maxLength":255},
        "direction":{"type":"string","enum":["forward","backward","up","down","left","right"]},
        "observation_ref":{"type":"string","format":"uuid"},
        "from_x":{"type":"integer","minimum":0,"maximum":16384},
        "from_y":{"type":"integer","minimum":0,"maximum":16384},
        "to_x":{"type":"integer","minimum":0,"maximum":16384},
        "to_y":{"type":"integer","minimum":0,"maximum":16384},
        "duration_ms":{"type":"integer","minimum":1,"maximum":2000},
        "destination":{"type":"string","enum":["back","home","recents"]}
    });
    let actions: &[(&str, &[&str], &[&str])] = &[
        ("observe", &[], &[]),
        ("capabilities", &[], &[]),
        ("screenshot", &[], &[]),
        (
            "query",
            &[
                "root_ref",
                "text_contains",
                "role",
                "max_nodes",
                "max_depth",
            ],
            &["max_nodes", "max_depth"],
        ),
        ("click", &["element_ref"], &["element_ref"]),
        ("long_click", &["element_ref"], &["element_ref"]),
        (
            "set_text",
            &["element_ref", "text"],
            &["element_ref", "text"],
        ),
        (
            "scroll",
            &["element_ref", "direction"],
            &["element_ref", "direction"],
        ),
        (
            "gesture",
            &[
                "observation_ref",
                "from_x",
                "from_y",
                "to_x",
                "to_y",
                "duration_ms",
            ],
            &[
                "observation_ref",
                "from_x",
                "from_y",
                "to_x",
                "to_y",
                "duration_ms",
            ],
        ),
        ("navigate", &["destination"], &["destination"]),
        ("launch_app", &["package_name"], &["package_name"]),
        (
            "wait_for",
            &["text", "present", "timeout_ms", "interval_ms"],
            &["text", "present", "timeout_ms", "interval_ms"],
        ),
    ];
    let variants: Vec<_> = actions.iter().map(|(action, names, required)| {
        let mut properties = serde_json::Map::new();
        properties.insert("action".into(), json!({"type":"string","const":action}));
        for name in *names { properties.insert((*name).into(), fields[name].clone()); }
        let mut required_fields = vec!["action"];
        required_fields.extend_from_slice(required);
        json!({"type":"object","properties":properties,"required":required_fields,"additionalProperties":false})
    }).collect();
    vec![json!({
        "name":"pab_mobile_control",
        "description":"Control an authenticated Android device. Read capabilities first for version 2 actions: bounded query (optional subtree, text and exact Android class), native long_click/scroll, launch_app (launcher packages only), and wait_for (text substring, observation only, bounded timeout). Legacy actions: observe, click, set_text, gesture, navigate (back/home/recents), screenshot. Use current element_ref or observation_ref; window changes and reconnect expire them. Preserve request_id: poll get when running, same ID/command deduplicates, changed parameters conflict. cancel only stops queued work; dispatched effects cannot be rolled back. Platform acknowledgement is not business success: observe again to verify. Never replay unconfirmed input with a new ID. Screen payloads are transient. Screenshots return MCP image content; Android 9/10 need local capture consent.",
        "inputSchema":{"type":"object","properties":{
            "device_code":{"type":"string","pattern":"^[0-9]{9}$"},
            "operation":{"type":"string","enum":["execute","get","cancel"]},
            "request_id":{"type":"string","format":"uuid"},
            "command":{"oneOf":variants}
        },"required":["device_code","operation","request_id"],"additionalProperties":false},
        "annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":true,"openWorldHint":false}
    })]
}

pub fn parse(args: &Value) -> Result<MobileRequest, String> {
    let mut object = args
        .as_object()
        .cloned()
        .ok_or("arguments must be an object")?;
    object.remove("device_code");
    let request: MobileRequest =
        serde_json::from_value(Value::Object(object)).map_err(|_| "invalid mobile request")?;
    if let MobileRequest::Execute { command, .. } = &request {
        command.validate().map_err(str::to_owned)?;
    }
    Ok(request)
}

pub async fn call(
    runtime: &BridgeRuntime,
    args: &Value,
) -> Result<(Value, Option<ContentBlock>), String> {
    let request = parse(args)?;
    let device = super::mcp_tools::resolve_target(runtime, args).await?;
    let (reply, image) = runtime
        .mobile(device, request)
        .await
        .map_err(|e| e.to_string())?;
    let content = image.map(|bytes| ContentBlock::image(STANDARD.encode(bytes), "image/jpeg"));
    let path = runtime
        .connection_path(device)
        .await
        .map(|path| match path {
            pab_transport::ConnectionPath::Direct => "direct",
            pab_transport::ConnectionPath::Relay => "relay",
            pab_transport::ConnectionPath::Unknown => "unknown",
        });
    let metadata = json!({"device_ref":device,"operation":reply,"image_included":content.is_some(),"connection_path":path});
    Ok((metadata, content))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mobile_tools_reject_wrong_targets_and_ambiguous_commands() {
        let id = pab_protocol::RequestId::new();
        assert!(parse(&json!({"device_code":"123456789","operation":"execute","request_id":id,"command":{"action":"set_text","element_ref":id,"text":"中".repeat(1400)}})).is_err());
        assert!(parse(&json!({"device_code":"123456789","operation":"get","request_id":id,"command":{"action":"observe"}})).is_err());
        assert!(parse(&json!({"device_code":"123456789","operation":"execute","request_id":id,"command":{"action":"observe"}})).is_ok());
    }
}
