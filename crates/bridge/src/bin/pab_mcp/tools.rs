use base64::{Engine, engine::general_purpose::STANDARD};
use pab_agent_core::{DataPaths, DataScope};
use pab_bridge::{BridgeLocalStore, BridgeRuntime};
use pab_protocol::{DeviceCode, OutputStream, RequestId, TaskId, TaskRef};
use serde_json::{Value, json};

pub(super) async fn call_tool(
    runtime: &BridgeRuntime,
    name: &str,
    arguments: &Value,
) -> Result<Value, String> {
    match name {
        "pab_list_network_connections"
        | "pab_resolve_dns"
        | "pab_list_sessions"
        | "pab_terminate_process"
        | "pab_list_services"
        | "pab_get_service"
        | "pab_service_control"
        | "pab_system_info"
        | "pab_list_disks"
        | "pab_list_processes"
        | "pab_get_process"
        | "pab_list_network_interfaces" => {
            super::mcp_system_query::call(runtime, name, arguments).await
        }
        "pab_file_stat"
        | "pab_file_read"
        | "pab_file_write"
        | "pab_file_patch"
        | "pab_file_search"
        | "pab_file_hash"
        | "pab_mkdir"
        | "pab_file_copy"
        | "pab_file_move"
        | "pab_file_delete"
        | "pab_archive_create"
        | "pab_archive_extract" => super::mcp_filesystem::call(runtime, name, arguments).await,
        "pab_list_devices" => list_local_devices().await,
        "pab_connect" => {
            let device_ref = resolve_target(runtime, arguments).await?;
            let context = runtime
                .connect_device(device_ref)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({
                "device_ref": device_ref,
                "target": context,
                "os_reminder": context.compact_reminder()
            }))
        }
        "pab_run_command" => {
            let device_ref = resolve_target(runtime, arguments).await?;
            let program = required_text(arguments, "program")?.to_owned();
            let args = arguments
                .get("args")
                .and_then(Value::as_array)
                .ok_or("args must be a string array")?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or("args must contain strings")
                })
                .collect::<Result<Vec<_>, _>>()?;
            let cwd = arguments
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let target = runtime
                .current_environment(device_ref)
                .await
                .map_err(|error| error.to_string())?;
            let snapshot = runtime
                .submit_command(device_ref, RequestId::new(), program, args, cwd)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({
                "task": snapshot,
                "operation_ref": { "device_code": arguments["device_code"], "operation_id": snapshot.request_id, "kind": "command", "task_id": snapshot.task_ref.task_id },
                "os_reminder": target.compact_reminder()
            }))
        }
        "pab_get_task" => {
            let task_ref = resolve_task(runtime, arguments).await?;
            let record = runtime
                .task(task_ref)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({
                "task_ref": task_ref,
                "snapshot": record.snapshot,
                "complete": record.is_complete(),
                "last_event_seq": record.last_event_seq,
                "stdout": record.stdout,
                "stderr": record.stderr
            }))
        }
        "pab_read_output" => {
            let task_ref = resolve_task(runtime, arguments).await?;
            let stream = match required_text(arguments, "stream")? {
                "stdout" => OutputStream::Stdout,
                "stderr" => OutputStream::Stderr,
                _ => return Err("stream must be stdout or stderr".to_owned()),
            };
            let offset = arguments.get("offset").and_then(Value::as_u64).unwrap_or(0);
            let (chunk, range) = runtime
                .read_output(task_ref, stream, offset, 64 * 1024)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({
                "text": String::from_utf8_lossy(&chunk.bytes),
                "offset": offset,
                "next_offset": offset + chunk.bytes.len() as u64,
                "range": range
            }))
        }
        "pab_list_directory" => {
            let device_ref = resolve_target(runtime, arguments).await?;
            let path = required_text(arguments, "path")?;
            let after = arguments.get("after").and_then(Value::as_str);
            let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(32);
            let limit = u16::try_from(limit).map_err(|_| "limit is too large")?;
            let target = runtime
                .current_environment(device_ref)
                .await
                .map_err(|error| error.to_string())?;
            let page = runtime
                .list_directory(device_ref, path, after, limit)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({ "page": page, "os_reminder": target.compact_reminder() }))
        }
        "pab_list_windows" => {
            let device_ref = resolve_target(runtime, arguments).await?;
            let target = runtime
                .current_environment(device_ref)
                .await
                .map_err(|error| error.to_string())?;
            let list = runtime
                .list_windows(device_ref)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({ "list": list, "os_reminder": target.compact_reminder() }))
        }
        "pab_desktop_input" => {
            let device_ref = resolve_target(runtime, arguments).await?;
            let event: pab_protocol::DesktopInputEvent =
                serde_json::from_value(arguments.get("event").cloned().ok_or("event is required")?)
                    .map_err(|error| error.to_string())?;
            let target = runtime
                .current_environment(device_ref)
                .await
                .map_err(|error| error.to_string())?;
            runtime
                .desktop_input(device_ref, event)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({ "applied": true, "os_reminder": target.compact_reminder() }))
        }
        "pab_open_terminal" => {
            let device_ref = resolve_target(runtime, arguments).await?;
            let cols = terminal_size(arguments, "cols", 80)?;
            let rows = terminal_size(arguments, "rows", 24)?;
            let target = runtime
                .current_environment(device_ref)
                .await
                .map_err(|error| error.to_string())?;
            let opened = runtime
                .open_terminal(device_ref, cols, rows)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({
                "session_id": opened.session_id,
                "shell": opened.shell,
                "cols": opened.cols,
                "rows": opened.rows,
                "os_reminder": target.compact_reminder()
            }))
        }
        "pab_terminal_input" => {
            let id = terminal_id(arguments)?;
            let data = required_text(arguments, "data")?;
            runtime
                .terminal_input(id, data.as_bytes())
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({ "session_id": id, "accepted_bytes": data.len() }))
        }
        "pab_terminal_read" => {
            let id = terminal_id(arguments)?;
            let output = runtime
                .terminal_read(id)
                .await
                .map_err(|error| error.to_string())?;
            if output.bytes.windows(4).any(|bytes| bytes == b"\x1b[6n") {
                runtime
                    .terminal_input(id, b"\x1b[1;1R")
                    .await
                    .map_err(|error| error.to_string())?;
            }
            Ok(json!({
                "session_id": id,
                "text": String::from_utf8_lossy(&output.bytes),
                "base64": STANDARD.encode(&output.bytes),
                "offset": output.offset,
                "next_offset": output.next_offset,
                "retained_from": output.retained_from,
                "ended": output.ended
            }))
        }
        "pab_terminal_resize" => {
            let id = terminal_id(arguments)?;
            let cols = terminal_size(arguments, "cols", 80)?;
            let rows = terminal_size(arguments, "rows", 24)?;
            runtime
                .terminal_resize(id, cols, rows)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({ "session_id": id, "cols": cols, "rows": rows }))
        }
        "pab_terminal_close" => {
            let id = terminal_id(arguments)?;
            runtime
                .terminal_close(id)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({ "session_id": id, "closed": true }))
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

pub(super) async fn list_local_devices() -> Result<Value, String> {
    let database = DataPaths::for_scope(DataScope::User)
        .map_err(|error| error.to_string())?
        .bridge_database();
    let store = BridgeLocalStore::open(&database)
        .await
        .map_err(|error| error.to_string())?;
    let devices = store
        .remembered_devices()
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|device| {
            json!({
                "device_code": device.code,
                "name": device.alias,
                "os": device.os_family,
                "os_reminder": device.os_reminder
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({ "devices": devices }))
}

pub(super) async fn resolve_target(
    runtime: &BridgeRuntime,
    arguments: &Value,
) -> Result<pab_protocol::DeviceRef, String> {
    let code: DeviceCode = required_text(arguments, "device_code")?
        .parse()
        .map_err(|error: pab_protocol::DeviceCodeError| error.to_string())?;
    runtime
        .resolve_cached_device_code(code)
        .await
        .map_err(|error| error.to_string())
}

async fn resolve_task(runtime: &BridgeRuntime, arguments: &Value) -> Result<TaskRef, String> {
    let device_ref = resolve_target(runtime, arguments).await?;
    let task_id: TaskId = required_text(arguments, "task_id")?
        .parse()
        .map_err(|_| "invalid task_id".to_owned())?;
    Ok(TaskRef {
        device_ref,
        task_id,
    })
}

fn required_text<'a>(arguments: &'a Value, key: &str) -> Result<&'a str, String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing {key}"))
}

fn terminal_id(arguments: &Value) -> Result<RequestId, String> {
    required_text(arguments, "session_id")?
        .parse()
        .map_err(|_| "invalid terminal session ID".to_owned())
}

fn terminal_size(arguments: &Value, key: &str, default: u16) -> Result<u16, String> {
    let value = arguments
        .get(key)
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(default));
    u16::try_from(value).map_err(|_| format!("invalid {key}"))
}
