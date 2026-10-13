use pab_bridge::BridgeRuntime;
use pab_protocol::RequestId;
use serde_json::{Value, json};

pub(super) fn handles(name: &str) -> bool {
    matches!(
        name,
        "pab_port_forward_open" | "pab_port_forward_status" | "pab_port_forward_close"
    )
}

pub(super) fn tools() -> Vec<Value> {
    let reference = json!({"type":"string","format":"uuid"});
    vec![
        json!({
            "name":"pab_port_forward_open",
            "description":"Open a TCP listener on 127.0.0.1 of the computer running this Pixels MCP process. Each local client gets a separate stream over the existing authenticated device connection to remote_host:remote_port as seen by the target Executor. Use this for remote development servers, databases and debug ports. The returned local address is NOT reachable from an isolated Agent container or another machine. No remote connection is tested until a local client connects. Repeating the same arguments returns the existing listener. Call close when done.",
            "inputSchema":{"type":"object","properties":{
                "device_code":{"type":"string","pattern":"^[0-9]{9}$"},
                "remote_host":{"type":"string","minLength":1,"maxLength":253,"default":"127.0.0.1"},
                "remote_port":{"type":"integer","minimum":1,"maximum":65535},
                "local_port":{"type":"integer","minimum":0,"maximum":65535,"default":0}
            },"required":["device_code","remote_port"],"additionalProperties":false}
        }),
        json!({
            "name":"pab_port_forward_status",
            "description":"Read this MCP session's TCP forwarding listener state, connection counters, bytes, route and most recent failure. Listening means the local socket exists; it does not prove the remote target is currently reachable.",
            "inputSchema":{"type":"object","properties":{"forward_id":reference},"required":["forward_id"],"additionalProperties":false}
        }),
        json!({
            "name":"pab_port_forward_close",
            "description":"Close this MCP session's TCP forwarding listener and all its active client connections. Terminal status is retained briefly for repeated close/status calls.",
            "inputSchema":{"type":"object","properties":{"forward_id":{"type":"string","format":"uuid"}},"required":["forward_id"],"additionalProperties":false}
        }),
    ]
}

pub(super) async fn call(
    runtime: &BridgeRuntime,
    name: &str,
    args: &Value,
) -> Result<Value, String> {
    let status = match name {
        "pab_port_forward_open" => {
            let device = super::mcp_tools::resolve_target(runtime, args).await?;
            let host = match args.get("remote_host") {
                None => "127.0.0.1",
                Some(value) => value.as_str().ok_or("remote_host must be a string")?,
            }
            .to_owned();
            let remote_port = args["remote_port"]
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
                .filter(|port| *port != 0)
                .ok_or("remote_port must be 1..65535")?;
            let local_port = match args.get("local_port") {
                None => 0,
                Some(value) => value
                    .as_u64()
                    .and_then(|port| u16::try_from(port).ok())
                    .ok_or("local_port must be 0..65535")?,
            };
            runtime
                .port_forward_open(device, host, remote_port, local_port)
                .await
        }
        "pab_port_forward_status" | "pab_port_forward_close" => {
            let id: RequestId = args["forward_id"]
                .as_str()
                .ok_or("forward_id required")?
                .parse()
                .map_err(|_| "invalid forward_id")?;
            if name == "pab_port_forward_status" {
                runtime.port_forward_status(id).await
            } else {
                runtime.port_forward_close(id).await
            }
        }
        _ => return Err("unknown port forwarding tool".into()),
    }
    .map_err(|error| error.to_string())?;
    serde_json::to_value(status).map_err(|error| error.to_string())
}
