use serde_json::{Value, json};

pub(super) fn tools() -> Vec<Value> {
    vec![
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
            "List visible windows in the target device's interactive desktop session. Requires a connected local desktop helper; headless devices report unsupported.",
            json!({
                "type": "object",
                "properties": { "device_code": { "type": "string", "pattern": "^[0-9]{9}$" } },
                "required": ["device_code"],
                "additionalProperties": false
            }),
        ),
        tool(
            "pab_capture_screenshot",
            "Capture the target device's interactive desktop as PNG binary data and save it to a new absolute local file. The result includes size, dimensions, and SHA-256; the screenshot bytes are not stored in the audit database. Requires a connected desktop helper.",
            json!({
                "type": "object",
                "properties": {
                    "device_code": { "type": "string", "pattern": "^[0-9]{9}$" },
                    "destination": { "type": "string" }
                },
                "required": ["device_code", "destination"],
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
            "Upload a binary file to a selected device. Paths must be absolute. Set overwrite=true to replace an existing regular file after verification.",
            file_tool_schema(),
        ),
        tool(
            "pab_download_file",
            "Download a binary file from a selected device. Paths must be absolute. Set overwrite=true to replace an existing regular file after verification.",
            file_tool_schema(),
        ),
    ]
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
            "source": { "type": "string" },
            "destination": { "type": "string" },
            "overwrite": { "type": "boolean", "default": false }
        },
        "required": ["device_code", "source", "destination"],
        "additionalProperties": false
    })
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}
