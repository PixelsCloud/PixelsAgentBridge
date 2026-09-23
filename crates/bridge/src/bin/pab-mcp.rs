use std::{
    env,
    io::{self, BufRead, Write},
    path::PathBuf,
    sync::Arc,
};

use pab_agent_core::{DataPaths, DataScope};
use pab_bridge::{
    BridgeConfig, BridgeRuntime, BridgeRuntimeConfig, DevicePasswordProvider,
    DirectoryDevicePasswordProvider, FileDevicePasswordProvider, MemoryDevicePasswordProvider,
};
use pab_protocol::{DeviceCode, OutputStream, RequestId, TaskId, TaskRef};
use serde_json::{Value, json};
use tokio::sync::mpsc;

const PROTOCOL_VERSION: &str = "2025-11-25";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() {
    let log_root = match DataPaths::for_scope(DataScope::User) {
        Ok(paths) => paths.root().to_path_buf(),
        Err(error) => {
            eprintln!("pab-mcp: {error}");
            std::process::exit(1);
        }
    };
    if let Err(error) = pab_logging::init("mcp", &log_root) {
        eprintln!("pab-mcp: {error}");
        std::process::exit(1);
    }
    if let Err(error) = run().await {
        tracing::error!(%error, "MCP process failed");
        eprintln!("pab-mcp: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    if env::args_os().len() != 1 {
        return Err("pab-mcp accepts no command-line arguments".into());
    }
    let guest = env::var("PAB_MCP_GUEST").as_deref() != Ok("0");
    let config = if guest {
        BridgeConfig::register_guest_from_env().await?
    } else {
        BridgeConfig::from_env()?
    };
    let paths = DataPaths::for_scope(DataScope::User)?;
    let database_path = env::var_os("PAB_BRIDGE_DATABASE")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.bridge_database());
    let fallback: Option<Box<dyn DevicePasswordProvider>> =
        if let Some(directory) = env::var_os("PAB_DEVICE_PASSWORD_DIR") {
            Some(Box::new(DirectoryDevicePasswordProvider::new(directory)))
        } else {
            env::var_os("PAB_DEVICE_PASSWORD_FILE").map(|file| {
                Box::new(FileDevicePasswordProvider::new(file)) as Box<dyn DevicePasswordProvider>
            })
        };
    let passwords = Arc::new(MemoryDevicePasswordProvider::new(fallback));
    let runtime = Arc::new(
        BridgeRuntime::start(
            config,
            BridgeRuntimeConfig::new(database_path),
            passwords.clone(),
        )
        .await?,
    );
    let (sender, mut receiver) = mpsc::channel::<String>(32);
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            match line {
                Ok(line) => {
                    if sender.blocking_send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    while let Some(line) = receiver.recv().await {
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => dispatch(&runtime, request).await,
            Err(_) => Some(rpc_error(Value::Null, -32700, "invalid JSON")),
        };
        if let Some(response) = response {
            let mut stdout = io::stdout().lock();
            serde_json::to_writer(&mut stdout, &response)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }
    Arc::try_unwrap(runtime)
        .map_err(|_| "MCP still holds the Bridge Runtime")?
        .shutdown()
        .await?;
    Ok(())
}

async fn dispatch(runtime: &BridgeRuntime, request: Value) -> Option<Value> {
    let id = request.get("id")?.clone();
    let method = request.get("method")?.as_str()?;
    let result = match method {
        "initialize" => json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "pixels-agent-bridge",
                "version": SERVER_VERSION
            },
            "instructions": "Select a device by its 9-digit code. Call pab_connect before commands and preserve its verified OS/shell context across context compaction. Windows, Linux and macOS have different commands. Use pab_run_command for native OS fallback. The device password stays in a local protected file; never pass it as a tool argument."
        }),
        "ping" => json!({}),
        "tools/list" => json!({
            "tools": tools()
        }),
        "tools/call" => {
            let Some(params) = request.get("params") else {
                return Some(rpc_error(id, -32602, "missing tool parameters"));
            };
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(rpc_error(id, -32602, "missing tool name"));
            };
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let result = call_tool(runtime, name, &arguments).await;
            match result {
                Ok(value) => json!({
                    "content": [{
                        "type": "text",
                        "text": value.to_string()
                    }],
                    "structuredContent": value,
                    "isError": false
                }),
                Err(error) => json!({
                    "content": [{
                        "type": "text",
                        "text": error
                    }],
                    "isError": true
                }),
            }
        }
        _ => return Some(rpc_error(id, -32601, "method not found")),
    };
    Some(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    }))
}

async fn call_tool(
    runtime: &BridgeRuntime,
    name: &str,
    arguments: &Value,
) -> Result<Value, String> {
    match name {
        "pab_list_devices" => {
            let devices = runtime
                .list_devices()
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({ "devices": devices }))
        }
        "pab_connect" => {
            let device_ref = resolve_target(runtime, arguments).await?;
            let context = runtime
                .current_environment(device_ref)
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
        _ => Err(format!("unknown tool: {name}")),
    }
}

async fn resolve_target(
    runtime: &BridgeRuntime,
    arguments: &Value,
) -> Result<pab_protocol::DeviceRef, String> {
    let code: DeviceCode = required_text(arguments, "device_code")?
        .parse()
        .map_err(|error: pab_protocol::DeviceCodeError| error.to_string())?;
    runtime
        .resolve_device_code(code)
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

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

fn tools() -> Vec<Value> {
    vec![
        tool(
            "pab_list_devices",
            "List account devices available in the current workspace.",
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
    ]
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}
