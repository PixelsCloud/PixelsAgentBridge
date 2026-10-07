use base64::{Engine, engine::general_purpose::STANDARD};
use pab_agent_core::{DataPaths, DataScope};
use pab_bridge::output_text::{OutputEncoding, decode_output};
use pab_bridge::{BridgeLocalStore, BridgeRuntime};
use pab_protocol::{DeviceCode, OutputStream, RequestId, TaskId, TaskRef};
use serde_json::{Value, json};

pub(super) async fn call_tool(
    runtime: &BridgeRuntime,
    name: &str,
    arguments: &Value,
) -> Result<Value, String> {
    match name {
        "pab_list_containers"
        | "pab_get_container"
        | "pab_container_logs"
        | "pab_container_control"
        | "pab_git_status"
        | "pab_git_diff"
        | "pab_git_log"
        | "pab_git_commit"
        | "pab_git_checkout"
        | "pab_git_fetch"
        | "pab_git_pull"
        | "pab_git_push" => super::mcp_system_query::call(runtime, name, arguments).await,
        "pab_list_network_connections"
        | "pab_resolve_dns"
        | "pab_list_sessions"
        | "pab_list_execution_contexts"
        | "pab_list_apps"
        | "pab_launch_app"
        | "pab_open_file"
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
            let encoding = output_encoding(arguments)?;
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
            let options = pab_protocol::CommandOptions {
                execution: serde_json::from_value(
                    arguments
                        .get("execution")
                        .cloned()
                        .unwrap_or(json!({"mode":"service"})),
                )
                .map_err(|e| e.to_string())?,
                env: serde_json::from_value(arguments.get("env").cloned().unwrap_or(json!({})))
                    .map_err(|e| e.to_string())?,
                stdin_text: arguments["stdin_text"].as_str().map(str::to_owned),
                timeout_ms: arguments["timeout_ms"].as_u64(),
            };
            options.validate().map_err(str::to_owned)?;
            let id = arguments["request_id"]
                .as_str()
                .map(str::parse::<RequestId>)
                .transpose()
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            let command = pab_protocol::CommandTaskSpec {
                display_summary: format!("{}{}", program, if args.is_empty() { "" } else { " …" })
                    .chars()
                    .take(512)
                    .collect(),
                program,
                args,
                cwd,
                options,
                expected_environment: pab_protocol::ExpectedEnvironment {
                    os_family: target.execution.os_family,
                    environment_revision: target.execution.environment_revision.clone(),
                },
            };
            let snapshot = runtime
                .submit_command_spec(device_ref, id, command)
                .await
                .map_err(|e| e.to_string())?;
            let mut wait_args = arguments.clone();
            wait_args["wait_until"] = json!("complete");
            let mut result = super::mcp_waiting::wait_value(&wait_args, || async {
                let record = runtime.task(snapshot.task_ref).await.map_err(|e| e.to_string())?;
                Ok(json!({"task":record.snapshot,"complete":record.is_complete(),"stdout":record.stdout,"stderr":record.stderr,
                    "operation_ref":{"device_code":arguments["device_code"],"operation_id":id,"kind":"command","task_id":snapshot.task_ref.task_id},"os_reminder":target.compact_reminder()}))
            }).await?;
            if result["complete"] == true && super::mcp_waiting::wait_ms(arguments) > 0 {
                let record = runtime
                    .task(snapshot.task_ref)
                    .await
                    .map_err(|e| e.to_string())?;
                for (name, stream, range) in [
                    ("stdout", OutputStream::Stdout, record.stdout),
                    ("stderr", OutputStream::Stderr, record.stderr),
                ] {
                    let offset = encoding
                        .align_start(
                            range
                                .available_to
                                .saturating_sub(8192)
                                .max(range.retained_from),
                        )
                        .min(range.available_to);
                    let (chunk, actual_range) = runtime
                        .read_output(snapshot.task_ref, stream, offset, 8192)
                        .await
                        .map_err(|e| e.to_string())?;
                    let mut preview = output_result(
                        arguments,
                        offset,
                        offset,
                        &chunk.bytes,
                        &actual_range,
                        encoding,
                    );
                    preview["truncated"] = json!(offset > 0);
                    result["output"][name] = preview;
                }
            }
            Ok(result)
        }
        "pab_get_task" => {
            let task_ref = resolve_task(runtime, arguments).await?;
            let record = runtime.task(task_ref).await.map_err(|e| e.to_string())?;
            if !record.is_complete() {
                // Once per call, not once per wait poll. Observe an accepted
                // task only; this cannot resubmit its command.
                let _ = runtime.follow_task(task_ref).await;
            }
            super::mcp_waiting::wait_value(arguments, || async {
                let record = runtime.task(task_ref).await.map_err(|e|e.to_string())?;
                Ok(json!({"task_ref":task_ref,"snapshot":record.snapshot,"complete":record.is_complete(),"last_event_seq":record.last_event_seq,"stdout":record.stdout,"stderr":record.stderr}))
            }).await
        }
        "pab_read_output" => read_output(runtime, arguments).await,

        "pab_list_directory" => {
            let execution = serde_json::from_value(
                arguments
                    .get("execution")
                    .cloned()
                    .unwrap_or(json!({"mode":"service"})),
            )
            .map_err(|error| error.to_string())?;
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
                .list_directory_as(device_ref, path, after, limit, execution)
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
            if arguments.get("actions").is_some() || arguments.get("monitor_input").is_some() {
                return super::mcp_desktop::call(runtime, name, arguments).await;
            }
            if ["window_ref", "timeout_ms", "request_id"]
                .iter()
                .any(|key| arguments.get(key).is_some())
            {
                return Err(
                    "batch parameters require actions; legacy event cannot use them".into(),
                );
            }
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
            let execution = serde_json::from_value(
                arguments
                    .get("execution")
                    .cloned()
                    .unwrap_or(json!({"mode":"service"})),
            )
            .map_err(|e| e.to_string())?;
            let target = runtime
                .current_environment(device_ref)
                .await
                .map_err(|error| error.to_string())?;
            let opened = runtime
                .open_terminal_as(device_ref, cols, rows, execution)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({
                "session_id": opened.session_id,
                "shell": opened.shell,
                "startup": opened.startup,
                "cols": opened.cols,
                "rows": opened.rows,
                "execution_identity": opened.identity,
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

async fn read_output(runtime: &BridgeRuntime, args: &Value) -> Result<Value, String> {
    let encoding = output_encoding(args)?;
    if let Some(offset) = args["offset"].as_u64() {
        encoding.validate_offset(offset)?;
    }
    let task_ref = resolve_task(runtime, args).await?;
    let stream = match required_text(args, "stream")? {
        "stdout" => OutputStream::Stdout,
        "stderr" => OutputStream::Stderr,
        _ => return Err("invalid stream".into()),
    };
    if args.get("offset").is_some() && args.get("tail_bytes").is_some() {
        return Err("offset and tail_bytes are mutually exclusive".into());
    }
    let maximum = args["max_bytes"].as_u64().unwrap_or(65536).min(65536) as u32;
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_millis(super::mcp_waiting::wait_ms(args));
    let mut requested = args["offset"].as_u64();
    loop {
        let record = runtime.task(task_ref).await.map_err(|e| e.to_string())?;
        let range = match stream {
            OutputStream::Stdout => record.stdout,
            OutputStream::Stderr => record.stderr,
        };
        let wanted = *requested.get_or_insert_with(|| {
            args["tail_bytes"]
                .as_u64()
                .map(|n| range.available_to.saturating_sub(n))
                .unwrap_or(0)
        });
        let offset = encoding
            .align_start(wanted.max(range.retained_from))
            .min(range.available_to);
        let (chunk, actual_range) = runtime
            .read_output(task_ref, stream, offset, maximum)
            .await
            .map_err(|e| e.to_string())?;
        let mut result = output_result(args, wanted, offset, &chunk.bytes, &actual_range, encoding);
        if result["next_offset"].as_u64().unwrap_or(offset) > offset
            || actual_range.complete
            || tokio::time::Instant::now() >= deadline
        {
            result["wait_expired"] = json!(
                result["next_offset"] == json!(offset)
                    && !actual_range.complete
                    && super::mcp_waiting::wait_ms(args) > 0
            );
            return Ok(result);
        }
        tokio::time::sleep_until(
            deadline.min(tokio::time::Instant::now() + std::time::Duration::from_millis(100)),
        )
        .await;
    }
}

fn output_encoding(args: &Value) -> Result<OutputEncoding, String> {
    serde_json::from_value(args.get("encoding").cloned().unwrap_or(json!("utf8")))
        .map_err(|e| format!("invalid output encoding: {e}"))
}

fn output_result(
    args: &Value,
    wanted: u64,
    offset: u64,
    bytes: &[u8],
    range: &pab_protocol::OutputRange,
    encoding: OutputEncoding,
) -> Value {
    let decoded = decode_output(
        bytes,
        encoding,
        range.complete && offset + bytes.len() as u64 >= range.available_to,
    );
    let text = &decoded.text;
    let filtered = args["contains"].as_str().map(|pattern| {
        text.lines()
            .filter(|line| line.contains(pattern))
            .collect::<Vec<_>>()
            .join("\n")
    });
    let mut result = json!({"text":filtered.as_deref().unwrap_or(text),"encoding":encoding,"offset":offset,"next_offset":offset+decoded.consumed as u64,"range":range,
        "gap":if offset>wanted {Some(json!({"from":wanted,"to":offset}))}else{None},
        "decoding_replacements":decoded.replacements,"pending_bytes":decoded.pending_bytes,
        "start_boundary_unverified":offset>0,
        "filter_scope":if filtered.is_some(){Some("returned_chunk_lines")}else{None},
        "wait_expired":decoded.consumed==0 && !range.complete && super::mcp_waiting::wait_ms(args)>0});
    if args["include_base64"] == true {
        result["base64"] = json!(STANDARD.encode(&bytes[..decoded.consumed]));
    }
    result
}

#[cfg(test)]
mod output_tests {
    use super::*;
    #[test]
    fn explicit_encoding_raw_bytes_and_split_characters_are_consistent() {
        let range = pab_protocol::OutputRange {
            retained_from: 0,
            available_to: 4,
            complete: false,
        };
        let first = output_result(
            &json!({"include_base64":true,"contains":"中"}),
            0,
            0,
            &[0xd6, 0xd0, 0xce],
            &range,
            OutputEncoding::Gbk,
        );
        assert_eq!(first["text"], "中");
        assert_eq!(first["next_offset"], 2);
        assert_eq!(first["pending_bytes"], 1);
        assert_eq!(first["base64"], STANDARD.encode([0xd6, 0xd0]));
        let last = output_result(
            &json!({"include_base64":true}),
            2,
            2,
            &[0xce, 0xc4],
            &pab_protocol::OutputRange {
                complete: true,
                ..range
            },
            OutputEncoding::Gbk,
        );
        assert_eq!(last["text"], "文");
        assert_eq!(last["next_offset"], 4);
        assert_eq!(last["pending_bytes"], 0);
        assert!(output_encoding(&json!({"encoding":"guess"})).is_err());
    }
    #[test]
    fn filtered_output_advances_over_all_bytes_and_reports_retention_gaps_and_invalid_utf8() {
        let bytes = "skip\n保留 😀\n".as_bytes();
        let range = pab_protocol::OutputRange {
            retained_from: 100,
            available_to: 100 + bytes.len() as u64,
            complete: false,
        };
        let result = output_result(
            &json!({"contains":"保留"}),
            0,
            100,
            bytes,
            &range,
            OutputEncoding::Utf8,
        );
        assert_eq!(result["text"], "保留 😀");
        assert_eq!(result["next_offset"], range.available_to);
        assert_eq!(result["gap"], json!({"from":0,"to":100}));
        assert_eq!(result["decoding_replacements"], false);
        let empty = output_result(
            &json!({"contains":"absent"}),
            100,
            100,
            bytes,
            &range,
            OutputEncoding::Utf8,
        );
        assert_eq!(empty["text"], "");
        assert_eq!(empty["next_offset"], range.available_to);
        let invalid = output_result(&json!({}), 100, 100, &[0xff], &range, OutputEncoding::Utf8);
        assert_eq!(invalid["decoding_replacements"], true);
        let pending = output_result(
            &json!({"wait_ms":5}),
            range.available_to,
            range.available_to,
            &[],
            &range,
            OutputEncoding::Utf8,
        );
        assert_eq!(pending["wait_expired"], true);
        assert_eq!(pending["next_offset"], range.available_to);
        let done = output_result(
            &json!({"wait_ms":5}),
            range.available_to,
            range.available_to,
            &[],
            &pab_protocol::OutputRange {
                complete: true,
                ..range
            },
            OutputEncoding::Utf8,
        );
        assert_eq!(done["wait_expired"], false);
    }
}
