use serde_json::{Value, json};

pub(super) fn tools() -> Vec<Value> {
    let mut tools = vec![
        tool(
            "pab_list_devices",
            "List devices previously connected from this computer, including their 9-digit codes, names and OS. Works while the server is offline.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        ),
        tool(
            "pab_connect",
            "Authenticate to one device and return its verified OS and shell context. Call this before native commands.",
            json!({
                "type": "object",
                "properties": {
                    "device_code": {
                        "type": "string",
                        "pattern": "^[0-9]{9}$"
                    }
                },
                "required": ["device_code"],
                "additionalProperties": false
            }),
        ),
        tool(
            "pab_run_command",
            "Run a native executable with an argv array on the selected device. This is the generic OS fallback.",
            json!({
                "type": "object",
                "properties": {
                    "device_code": {
                        "type": "string",
                        "pattern": "^[0-9]{9}$"
                    },
                    "program": {
                        "type": "string"
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "cwd": {
                        "type": "string"
                    }
                },
                "required": ["device_code", "program", "args"],
                "additionalProperties": false
            }),
        ),
        tool(
            "pab_get_task",
            "Read live task state, progress and output ranges.",
            json!({
                "type": "object",
                "properties": {
                    "device_code": {
                        "type": "string",
                        "pattern": "^[0-9]{9}$"
                    },
                    "task_id": {
                        "type": "string"
                    }
                },
                "required": ["device_code", "task_id"],
                "additionalProperties": false
            }),
        ),
        tool(
            "pab_read_output",
            "Read retained stdout or stderr from a task offset.",
            json!({
                "type": "object",
                "properties": {
                    "device_code": {
                        "type": "string",
                        "pattern": "^[0-9]{9}$"
                    },
                    "task_id": {
                        "type": "string"
                    },
                    "stream": {
                        "type": "string",
                        "enum": ["stdout", "stderr"]
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 0
                    }
                },
                "required": ["device_code", "task_id", "stream"],
                "additionalProperties": false
            }),
        ),
        tool(
            "pab_list_directory",
            "List one page of entries in an absolute directory on a selected device. Symlink entries are classified without following them for metadata. Use next_after for the next page.",
            json!({
                "type": "object",
                "properties": {
                    "device_code": { "type": "string", "pattern": "^[0-9]{9}$" },
                    "path": { "type": "string" },
                    "after": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 64, "default": 32 }
                },
                "required": ["device_code", "path"],
                "additionalProperties": false
            }),
        ),
        tool(
            "pab_list_windows",
            "List windows via xcap with title, process, geometry, minimized/maximized/focused state and opaque window_ref (or control_error if unavailable). References are bound to the current desktop helper, checked for handle reuse and invalid after window destruction/helper reconnect/desktop switch or marker loss. Returns a bounded 32 KiB snapshot, max 64 entries; use window_ref for focus/control/text. Requires capability v4 and upgraded helper; macOS enumeration only, Windows/X11 control. xcap enumeration filters may omit helper-owned, hidden or cloaked windows.",
            json!({"type":"object","properties":{"device_code":{"type":"string","pattern":"^[0-9]{9}$"},"request_id":{"type":"string","format":"uuid"}},"required":["device_code"],"additionalProperties":false}),
        ),
        tool(
            "pab_capture_screenshot",
            "Capture the current desktop or referenced window via xcap, encode once as JPEG at its captured resolution (quality 85 by default), and return the complete MCP image plus saved .jpg file. No resizing, image byte limit, adaptive quality, raw-frame cache or capture_id. Requires screenshot capability v3 and upgraded helper. window_ref comes from pab_list_windows and cannot combine with monitor_id/region; stale, minimized or changing windows fail. Desktop region is monitor-relative. Coordinate mapping describes capture time; no protected-window/headless guarantee. destination is optional, absolute and create-only; extension .jpg/.jpeg.",
            json!({
                "type": "object",
                "properties": {
                    "device_code": { "type": "string", "pattern": "^[0-9]{9}$" },
                    "destination": { "type": "string", "minLength": 1, "maxLength": 4096 },
                    "include_image": { "type": "boolean", "default": true },
                    "mode": { "type": "string", "enum": ["jpeg"], "default": "jpeg" },
                    "format": { "type": "string", "enum": ["jpeg"], "default": "jpeg" },
                    "window_ref": { "type": "string", "format": "uuid" },
                    "monitor_id": { "type": "integer", "minimum": 0, "maximum": 4294967295u64 },
                    "region": { "type": "object", "properties": {
                        "x": { "type": "integer", "minimum": 0, "maximum": 4294967295u64 },
                        "y": { "type": "integer", "minimum": 0, "maximum": 4294967295u64 },
                        "width": { "type": "integer", "minimum": 1, "maximum": 65535 },
                        "height": { "type": "integer", "minimum": 1, "maximum": 65535 }
                    }, "required": ["x", "y", "width", "height"], "additionalProperties": false },
                    "quality": { "type": "integer", "minimum": 30, "maximum": 95 }
                },
                "required": ["device_code"],
                "additionalProperties": false
            }),
        ),
        tool(
            "pab_desktop_input",
            "Send one input event to the target's current Windows desktop. All authenticated operators may input. The event type is mouse_move (x/y 0..65535), mouse_button (button left/right/middle, down), mouse_wheel (delta), key (virtual_key, down), or secure_attention (Windows service Ctrl+Alt+Delete request).",
            json!({
                "type": "object",
                "properties": {
                    "device_code": { "type": "string", "pattern": "^[0-9]{9}$" },
                    "event": { "type": "object" }
                },
                "required": ["device_code", "event"],
                "additionalProperties": false
            }),
        ),
        tool(
            "pab_open_terminal",
            "Open an interactive shell. Default execution is service; user requires a context_ref discovered on this connection. Identity is fixed for this session and never falls back. Subsequent calls use session_id. Disconnect closes the session; no automatic reopen or replay of input.",
            json!({
                "type": "object",
                "properties": {
                    "device_code": { "type": "string", "pattern": "^[0-9]{9}$" },
                    "cols": { "type": "integer", "minimum": 20, "maximum": 500, "default": 80 },
                    "rows": { "type": "integer", "minimum": 5, "maximum": 200, "default": 24 }
                },
                "required": ["device_code"],
                "additionalProperties": false
            }),
        ),
        tool(
            "pab_terminal_input",
            "Send UTF-8 bytes to an interactive terminal. Include a newline such as \\r when submitting a command. Input is audited.",
            json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "data": { "type": "string", "minLength": 1, "maxLength": 4096 }
                },
                "required": ["session_id", "data"],
                "additionalProperties": false
            }),
        ),
        tool(
            "pab_terminal_read",
            "Read the next chunk of terminal output. Returns UTF-8 text and exact base64 bytes; repeat until ended.",
            terminal_session_schema(),
        ),
        tool(
            "pab_terminal_resize",
            "Resize an interactive terminal.",
            json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "cols": { "type": "integer", "minimum": 20, "maximum": 500 },
                    "rows": { "type": "integer", "minimum": 5, "maximum": 200 }
                },
                "required": ["session_id", "cols", "rows"],
                "additionalProperties": false
            }),
        ),
        tool(
            "pab_terminal_close",
            "Close an interactive terminal and archive its final output.",
            terminal_session_schema(),
        ),
        tool(
            "pab_upload_file",
            "Queue a binary upload and immediately return operation_ref. Query pab_get_operation for progress/outcome. Reuse request_id to deduplicate; never resubmit an unconfirmed result with a new ID. Absolute paths; overwrite replaces only after verification; wait_ms optionally waits up to 5000 ms.",
            file_tool_schema(),
        ),
        tool(
            "pab_download_file",
            "Queue a binary download and immediately return operation_ref. Query pab_get_operation for progress/outcome. Reuse request_id to deduplicate; never resubmit an unconfirmed result with a new ID. Absolute paths; overwrite replaces only after verification; wait_ms optionally waits up to 5000 ms.",
            file_tool_schema(),
        ),
        tool(
            "pab_get_operation",
            "Read durable transfer or command status by original request ID and device code. Works offline for cached records; unconfirmed transfers are checked in the background. Cached OS context is explicitly marked.",
            operation_schema(),
        ),
        tool(
            "pab_cancel_operation",
            "Request cancellation of an operation owned by this MCP session. cancel_requested is intent, not confirmation. Completion wins if publication already succeeded. Query the original operation for the outcome.",
            operation_schema(),
        ),
        tool(
            "pab_list_operations",
            "List this MCP session's operations, newest first, using a stable before cursor. Optional device and state filters.",
            json!({
                "type": "object", "properties": {
                    "device_code": { "type": "string", "pattern": "^[0-9]{9}$" },
                    "state": { "type": "string", "enum": ["running", "cancel_requested", "completed", "failed", "cancelled"] },
                    "before": { "type": "string", "maxLength": 128 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
                }, "additionalProperties": false
            }),
        ),
        tool(
            "pab_disconnect",
            "Disconnect this MCP session from one device. Other MCP and Desktop connections are independent. Finish or cancel active transfers first; explicitly pab_connect to reconnect.",
            json!({
                "type": "object", "properties": { "device_code": { "type": "string", "pattern": "^[0-9]{9}$" } },
                "required": ["device_code"], "additionalProperties": false
            }),
        ),
    ];
    tools.extend(super::mcp_filesystem::tools());
    tools.extend(super::mcp_system_query::tools());
    tools.extend(super::mcp_git::tools());
    tools.extend(super::mcp_container::tools());
    tools.extend(super::mcp_desktop::tools());
    tools.extend(super::mcp_ui::tools());
    super::mcp_desktop::enhance_input_tool(
        tools
            .iter_mut()
            .find(|t| t["name"] == "pab_desktop_input")
            .unwrap(),
    );
    for entry in &mut tools {
        let name = entry["name"].as_str().unwrap().to_owned();
        let properties = entry["inputSchema"]["properties"].as_object_mut().unwrap();
        if matches!(
            name.as_str(),
            "pab_run_command"
                | "pab_get_task"
                | "pab_get_operation"
                | "pab_read_output"
                | "pab_connect"
        ) {
            properties.insert("wait_ms".into(),json!({"type":"integer","minimum":0,"maximum":30000,"default":if name=="pab_connect"{5000}else{0}}));
        }
        if matches!(name.as_str(), "pab_get_task" | "pab_get_operation") {
            properties.insert(
                "after_revision".into(),
                json!({"type":"string","pattern":"^[a-f0-9]{64}$"}),
            );
            properties.insert(
                "wait_until".into(),
                json!({"type":"string","enum":["change","complete"],"default":"change"}),
            );
        }
        if matches!(name.as_str(), "pab_run_command" | "pab_open_terminal") {
            properties.insert("execution".into(),json!({"oneOf":[
                {"type":"object","properties":{"mode":{"const":"service"}},"required":["mode"],"additionalProperties":false},
                {"type":"object","properties":{"mode":{"const":"user"},"context_ref":{"type":"string","format":"uuid"}},"required":["mode","context_ref"],"additionalProperties":false}
            ]}));
        }
        if name == "pab_run_command" {
            properties.insert(
                "request_id".into(),
                json!({"type":"string","format":"uuid"}),
            );
            properties.insert("env".into(),json!({"type":"object","maxProperties":64,"additionalProperties":{"type":"string"}}));
            properties.insert(
                "stdin_text".into(),
                json!({"type":"string","maxLength":16384}),
            );
            properties.insert(
                "timeout_ms".into(),
                json!({"type":"integer","minimum":1,"maximum":86400000}),
            );
        }
        if name == "pab_read_output" {
            properties.insert(
                "max_bytes".into(),
                json!({"type":"integer","minimum":4,"maximum":65536,"default":65536}),
            );
            properties.insert(
                "tail_bytes".into(),
                json!({"type":"integer","minimum":1,"maximum":65536}),
            );
            properties.insert(
                "contains".into(),
                json!({"type":"string","minLength":1,"maxLength":1024}),
            );
        }
        let extra = match name.as_str() {
            "pab_run_command" => {
                " Execution defaults to the service account. User execution requires command v3 and a user context_ref returned by pab_list_execution_contexts on this connection; unavailable contexts never fall back to service. Reuse request_id to observe the original task; changing execution conflicts. env/stdin_text/timeout_ms require command v2. stdin is UTF-8, maximum 16 KiB. wait_ms waits after remote acceptance; it does not stop the command. timeout_ms stops the direct child; descendants may remain. Output ranges are returned; use pab_read_output for bytes."
            }
            "pab_get_task" | "pab_get_operation" => {
                " wait_ms optionally waits up to 30s for change or completion. Reuse after_revision to avoid missing updates. Expiry returns observed facts, not a task failure."
            }
            "pab_read_output" => {
                " tail_bytes and offset are exclusive. max_bytes bounds scanned bytes. contains filters only lines/fragments in this returned chunk; no whole-log or cross-chunk match guarantee. next_offset advances over scanned bytes. gap reports discarded retained output. wait_ms waits for new bytes or EOF."
            }
            "pab_connect" => {
                " wait_ms defaults to 5000; if pending, returns connection_ref and connected=false. Call pab_connect again to observe the same in-flight attempt. At most 120s per attempt; pab_disconnect stops this session's attempt."
            }
            _ => "",
        };
        let description = entry["description"].as_str().unwrap().to_owned() + extra;
        entry["description"] = json!(description);
    }
    tools
}

pub(super) fn enabled_tools(
    settings: &pab_bridge::mcp_tool_settings::McpToolSettings,
) -> Vec<Value> {
    tools()
        .into_iter()
        .filter(|tool| {
            tool["name"]
                .as_str()
                .is_some_and(|name| settings.allows(name))
        })
        .collect()
}

fn terminal_session_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "session_id": { "type": "string" } },
        "required": ["session_id"],
        "additionalProperties": false
    })
}

fn file_tool_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "device_code": { "type": "string", "pattern": "^[0-9]{9}$" },
            "source": { "type": "string", "minLength": 1, "maxLength": 4096 },
            "destination": { "type": "string", "minLength": 1, "maxLength": 4096 },
            "overwrite": { "type": "boolean", "default": false },
            "request_id": { "type": "string", "format": "uuid" },
            "wait_ms": { "type": "integer", "minimum": 0, "maximum": 5000, "default": 0 }
        },
        "required": ["device_code", "source", "destination"],
        "additionalProperties": false
    })
}

fn operation_schema() -> Value {
    json!({ "type": "object", "properties": {
        "device_code": { "type": "string", "pattern": "^[0-9]{9}$" },
        "operation_id": { "type": "string", "format": "uuid" }
    }, "required": ["device_code", "operation_id"], "additionalProperties": false })
}

pub(super) fn validate_arguments(name: &str, args: &Value) -> Result<(), String> {
    let definition = tools()
        .into_iter()
        .find(|tool| tool["name"] == name)
        .ok_or_else(|| format!("unknown tool: {name}"))?;
    validate_value(args, &definition["inputSchema"], "arguments")?;
    if super::mcp_ui::handles(name) {
        super::mcp_ui::parse(name, args)?;
    }
    if name == "pab_desktop_input" {
        if args.get("actions").is_some() || args.get("monitor_input").is_some() {
            super::mcp_desktop::parse(name, args)?;
        } else {
            if ["window_ref", "timeout_ms", "request_id"]
                .iter()
                .any(|key| args.get(key).is_some())
            {
                return Err("batch parameters require actions".into());
            }
            serde_json::from_value::<pab_protocol::DesktopInputEvent>(
                args.get("event")
                    .cloned()
                    .ok_or("event or actions required")?,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    if matches!(name, "pab_file_read" | "pab_file_search" | "pab_file_patch") {
        super::mcp_filesystem::parse(name, args)?;
    }
    if matches!(name, "pab_run_command" | "pab_open_terminal") {
        let options = pab_protocol::CommandOptions {
            execution: serde_json::from_value(
                args.get("execution")
                    .cloned()
                    .unwrap_or(json!({"mode":"service"})),
            )
            .map_err(|e| e.to_string())?,
            env: args
                .get("env")
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()
                .map_err(|e| e.to_string())?
                .unwrap_or_default(),
            stdin_text: args["stdin_text"].as_str().map(str::to_owned),
            timeout_ms: args["timeout_ms"].as_u64(),
        };
        options.validate().map_err(str::to_owned)?;
    }
    if name == "pab_read_output" && args.get("offset").is_some() && args.get("tail_bytes").is_some()
    {
        return Err("offset and tail_bytes are mutually exclusive".into());
    }
    Ok(())
}

fn validate_value(value: &Value, schema: &Value, path: &str) -> Result<(), String> {
    let valid_type = match schema["type"].as_str() {
        Some("object") => value.is_object(),
        Some("array") => value.is_array(),
        Some("string") => value.is_string(),
        Some("boolean") => value.is_boolean(),
        Some("integer") => value.as_i64().is_some() || value.as_u64().is_some(),
        _ => true,
    };
    if !valid_type {
        return Err(format!("{path}: expected {}", schema["type"]));
    }
    if let Some(options) = schema["enum"].as_array()
        && !options.contains(value)
    {
        return Err(format!("{path}: unsupported value"));
    }
    if let Some(number) = value.as_f64()
        && (schema["minimum"].as_f64().is_some_and(|min| number < min)
            || schema["maximum"].as_f64().is_some_and(|max| number > max))
    {
        return Err(format!("{path}: outside the permitted range"));
    }
    if let Some(string) = value.as_str() {
        let count = string.chars().count() as u64;
        if schema["minLength"].as_u64().is_some_and(|min| count < min)
            || schema["maxLength"].as_u64().is_some_and(|max| count > max)
        {
            return Err(format!("{path}: invalid length"));
        }
        if schema["pattern"] == "^[0-9]{9}$"
            && (string.len() != 9 || !string.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(format!(
                "{path}: expected a 9-digit device code without spaces"
            ));
        }
        if schema["format"] == "uuid" && string.parse::<pab_protocol::RequestId>().is_err() {
            return Err(format!("{path}: expected a UUID"));
        }
        if schema["pattern"] == "^[a-f0-9]{64}$"
            && (string.len() != 64
                || !string
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            return Err(format!(
                "{path}: expected a revision returned by the previous query"
            ));
        }
    }
    if let Some(object) = value.as_object() {
        if schema["maxProperties"]
            .as_u64()
            .is_some_and(|max| object.len() > max as usize)
        {
            return Err(format!("{path}: too many properties"));
        }
        if let Some(required) = schema["required"].as_array() {
            for key in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(key) {
                    return Err(format!("{path}: missing {key}"));
                }
            }
        }
        for (key, value) in object {
            if let Some(child) = schema["properties"].get(key) {
                validate_value(value, child, &format!("{path}.{key}"))?;
            } else if schema["additionalProperties"] == false {
                return Err(format!("{path}: unknown field {key}"));
            } else if schema["additionalProperties"].is_object() {
                validate_value(
                    value,
                    &schema["additionalProperties"],
                    &format!("{path}.{key}"),
                )?;
            }
        }
    }
    if let Some(array) = value.as_array()
        && (schema["minItems"]
            .as_u64()
            .is_some_and(|min| array.len() < min as usize)
            || schema["maxItems"]
                .as_u64()
                .is_some_and(|max| array.len() > max as usize))
    {
        return Err(format!("{path}: invalid item count"));
    }
    if let Some(array) = value.as_array() {
        for (index, value) in array.iter().enumerate() {
            validate_value(value, &schema["items"], &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn terminal_user_selection_rejects_claimed_identity_and_desktop_mode() {
        let valid = serde_json::json!({"device_code":"123456789","execution":{"mode":"user","context_ref":pab_protocol::ExecutionContextRef::new()}});
        super::validate_arguments("pab_open_terminal", &valid).unwrap();
        for execution in [
            serde_json::json!({"mode":"user","username":"fixture"}),
            serde_json::json!({"mode":"service","context_ref":pab_protocol::ExecutionContextRef::new()}),
            serde_json::json!({"mode":"desktop_user","context_ref":pab_protocol::ExecutionContextRef::new()}),
        ] {
            let mut args = valid.clone();
            args["execution"] = execution;
            assert!(super::validate_arguments("pab_open_terminal", &args).is_err());
        }
    }
    use super::*;
    #[test]
    fn command_user_selection_accepts_only_discovered_reference_shape() {
        let mut args = json!({"device_code":"123456789","program":"whoami","args":[]});
        args["execution"] =
            json!({"mode":"user","context_ref":pab_protocol::ExecutionContextRef::new()});
        assert!(validate_arguments("pab_run_command", &args).is_ok());
        for value in [
            json!({"mode":"user","username":"alice"}),
            json!({"mode":"service","context_ref":pab_protocol::ExecutionContextRef::new()}),
            json!({"mode":"desktop_user","context_ref":pab_protocol::ExecutionContextRef::new()}),
            json!(null),
        ] {
            args["execution"] = value;
            assert!(validate_arguments("pab_run_command", &args).is_err());
        }
    }
    #[test]
    fn every_tool_has_exactly_one_group_and_filtered_schemas_are_unchanged() {
        use pab_bridge::mcp_tool_settings::{McpToolSettings, ToolGroup};
        let all = tools();
        assert_eq!(all.len(), 65);
        assert_eq!(enabled_tools(&McpToolSettings::default()), all);
        for group in ToolGroup::ALL {
            let settings = McpToolSettings {
                version: 1,
                enabled_groups: if group == ToolGroup::Core {
                    vec![group]
                } else {
                    vec![ToolGroup::Core, group]
                },
            };
            let filtered = enabled_tools(&settings);
            assert_eq!(
                filtered.len(),
                ToolGroup::Core.tools().len()
                    + if group == ToolGroup::Core {
                        0
                    } else {
                        group.tools().len()
                    }
            );
            for tool in &filtered {
                assert!(all.contains(tool));
            }
        }
        for group in ToolGroup::ALL {
            for name in group.tools() {
                assert!(all.iter().any(|t| t["name"] == *name), "{name}");
            }
        }
        for tool in all {
            assert!(ToolGroup::for_tool(tool["name"].as_str().unwrap()).is_some());
        }
    }
    #[test]
    fn catalog_rejects_invalid_arguments_before_accepting_work() {
        for args in [
            json!({"device_code":"123 456 789","operation_id":"bad"}),
            json!({"device_code":"123456789","operation_id":"bad"}),
            json!({"device_code":"123456789","operation_id":pab_protocol::RequestId::new(),"extra":true}),
        ] {
            assert!(validate_arguments("pab_get_operation", &args).is_err());
        }
        assert!(validate_arguments("pab_list_operations", &json!({"limit":0})).is_err());
        assert!(validate_arguments("pab_list_operations", &json!({"limit":1.5})).is_err());
        assert!(validate_arguments("pab_list_operations", &json!({"state":"invented"})).is_err());
        assert!(validate_arguments("pab_upload_file",&json!({"device_code":"123456789","source":"/tmp/source","destination":"/tmp/dest","wait_ms":5001})).is_err());
        assert!(
            validate_arguments(
                "pab_run_command",
                &json!({"device_code":"123456789","program":"echo","args":[9]})
            )
            .is_err()
        );
        assert!(validate_arguments("pab_disconnect", &json!({"device_code":"123456789"})).is_ok());
        assert!(validate_arguments("pab_list_operations", &json!({})).is_ok());
    }

    #[test]
    fn enhanced_arguments_reject_invalid_environment_and_waits_before_networking() {
        let base = json!({"device_code":"123456789","program":"echo","args":[]});
        for (key, value) in [
            ("env", json!({"NAME":3})),
            ("env", json!({"BAD=KEY":"value"})),
            ("stdin_text", json!("中".repeat(6000))),
            ("timeout_ms", json!(0)),
            ("wait_ms", json!(30001)),
            ("request_id", json!("invalid")),
        ] {
            let mut args = base.clone();
            args[key] = value;
            assert!(
                validate_arguments("pab_run_command", &args).is_err(),
                "{key}"
            );
        }
        let mut args = base;
        args["env"] = json!({"KEY":"中文"});
        args["timeout_ms"] = json!(5000);
        args["wait_ms"] = json!(30000);
        assert!(validate_arguments("pab_run_command", &args).is_ok());
        assert!(validate_arguments("pab_get_operation",&json!({"device_code":"123456789","operation_id":pab_protocol::RequestId::new(),"after_revision":"bad"})).is_err());
        assert!(validate_arguments("pab_read_output",&json!({"device_code":"123456789","task_id":pab_protocol::TaskId::new(),"stream":"stdout","tail_bytes":4,"offset":0})).is_err());
    }
}
