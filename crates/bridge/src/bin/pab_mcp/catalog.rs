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
            "Open an interactive shell on a selected device and return its verified OS context. Subsequent calls use session_id; do not assume commands from another OS work here.",
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
    tools
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
    validate_value(args, &definition["inputSchema"], "arguments")
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
    }
    if let Some(object) = value.as_object() {
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
    use super::*;
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
}
