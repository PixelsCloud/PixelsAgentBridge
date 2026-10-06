//! Shared real-stdio transport used by isolated and opt-in live regressions.
use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
    service::RunningService,
};
use serde_json::Value;
use std::{process::Stdio, time::Duration};
pub type Client = RunningService<RoleClient, ()>;

pub async fn start(mut command: tokio::process::Command) -> (Client, tokio::process::Child) {
    let mut child = command
        .env("PAB_MCP_GUEST", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("start test MCP");
    let client = tokio::time::timeout(
        Duration::from_secs(10),
        ().serve((child.stdout.take().unwrap(), child.stdin.take().unwrap())),
    )
    .await
    .expect("MCP initialize timeout")
    .expect("MCP initialize failed");
    (client, child)
}

pub async fn call(client: &Client, name: &str, args: Value, timeout: Duration) -> CallToolResult {
    // A timeout is NOT a remote failure and must never trigger mutation replay.
    tokio::time::timeout(
        timeout,
        client.call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        ),
    )
    .await
    .expect("tool result unconfirmed after transport timeout; inspect original request")
    .expect("tool transport failed; inspect original request before retry")
}

pub async fn stop(client: Client, mut child: tokio::process::Child) {
    client.cancel().await.expect("cancel MCP service");
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("MCP shutdown timeout")
        .expect("wait for MCP exit");
    assert!(status.success(), "MCP did not shut down cleanly: {status}");
}
