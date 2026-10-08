//! Read-only MCP status reporting. This channel never executes remote operations.
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures_util::{SinkExt, StreamExt};
use pab_protocol::{DeviceRef, ExecutionContext, OutputAvailability, RequestId, TaskProgress};
use serde::{Deserialize, Serialize};
use tokio::{sync::watch, task::JoinHandle};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

pub const REPORTING_PORT: u16 = 26035;
pub const REPORTING_PROTOCOL_VERSION: u16 = 2;
pub const MAX_REPORT_BYTES: usize = 1024 * 1024;
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(20);
pub const RECONNECT_INTERVAL: Duration = Duration::from_secs(3);

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceReport {
    pub device_ref: DeviceRef,
    pub device_code: Option<String>,
    pub name: Option<String>,
    pub alias: Option<String>,
    pub phase: String,
    pub connection_path: Option<String>,
    pub retry_in_ms: Option<u64>,
    pub last_error: Option<String>,
    pub changed_at_unix_ms: i64,
    pub environment: Option<ExecutionContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskReport {
    pub request_id: RequestId,
    pub task_id: Option<String>,
    pub device_ref: DeviceRef,
    pub capability: String,
    pub state: String,
    pub stage: Option<String>,
    pub progress: Option<TaskProgress>,
    pub created_at_unix_ms: i64,
    pub started_at_unix_ms: Option<i64>,
    pub finished_at_unix_ms: Option<i64>,
    pub exit_code: Option<i32>,
    pub error_code: Option<String>,
    pub output: OutputAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationReport {
    pub id: String,
    pub device_ref: DeviceRef,
    pub device_code: Option<String>,
    pub kind: String,
    pub direction: String,
    pub source: String,
    pub destination: String,
    pub state: String,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub started_at_unix_ms: i64,
    pub finished_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeReport {
    pub account: Option<AccountSyncReport>,
    pub session_id: String,
    pub tenant_id: String,
    pub identity: String,
    pub control_url: String,
    pub relay_urls: Vec<String>,
    pub control_phase: String,
    pub control_changed_at_unix_ms: i64,
    pub control_generation: u64,
    pub control_consecutive_failures: u32,
    pub control_retry_in_ms: Option<u64>,
    pub last_error: Option<String>,
    pub devices: Vec<DeviceReport>,
    pub tasks: Vec<TaskReport>,
    pub operations: Vec<OperationReport>,
    pub sampled_at_unix_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountSyncReport {
    pub relay_nodes: Vec<pab_protocol::RelayUserContextReceipt>,
    pub local_revision: u64,
    pub server_revision: Option<u64>,
    pub remote_revision: Option<u64>,
    pub policy_version: Option<u64>,
    pub user: Option<pab_protocol::UserAttribution>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolCallReport {
    pub id: Uuid,
    pub tool: String,
    pub started_at_unix_ms: i64,
    pub finished_at_unix_ms: Option<i64>,
    pub device_code: Option<String>,
    pub device_ref: Option<DeviceRef>,
    pub task_id: Option<String>,
    pub succeeded: Option<bool>,
    pub request_id: Option<String>,
    pub terminal_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpReport {
    pub protocol_version: u16,
    pub session_id: Uuid,
    pub process_id: u32,
    pub version: String,
    pub started_at_unix_ms: i64,
    pub os: String,
    pub architecture: String,
    pub client_name: Option<String>,
    pub client_version: Option<String>,
    pub active_calls: Vec<ToolCallReport>,
    pub recent_calls: Vec<ToolCallReport>,
    pub runtime: Option<RuntimeReport>,
    pub updated_at_unix_ms: i64,
}

impl McpReport {
    pub fn new() -> Self {
        let now = now_ms();
        Self {
            protocol_version: REPORTING_PROTOCOL_VERSION,
            session_id: Uuid::new_v4(),
            process_id: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            started_at_unix_ms: now,
            os: std::env::consts::OS.to_owned(),
            architecture: std::env::consts::ARCH.to_owned(),
            client_name: None,
            client_version: None,
            active_calls: Vec::new(),
            recent_calls: Vec::new(),
            runtime: None,
            updated_at_unix_ms: now,
        }
    }
    pub fn valid(&self) -> bool {
        self.protocol_version == REPORTING_PROTOCOL_VERSION
            && self.process_id > 0
            && self.version.len() <= 128
            && self.active_calls.len() <= 1024
            && self.recent_calls.len() <= 100
            && self.runtime.as_ref().is_none_or(|r| {
                r.devices.len() <= 4096 && r.tasks.len() <= 4096 && r.operations.len() <= 4096
            })
    }
}

impl Default for McpReport {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedMcp {
    pub report: McpReport,
    pub peer_address: String,
    pub connected_at_unix_ms: i64,
    pub last_seen_at_unix_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpReportingStatus {
    pub revision: u64,
    pub running: bool,
    pub listen_address: String,
    pub error: Option<String>,
    pub count: usize,
    pub clients: Vec<ConnectedMcp>,
}

/// Writers never wait for network I/O. The watch channel coalesces progress updates.
#[derive(Clone)]
pub struct McpReporter {
    report: Arc<watch::Sender<McpReport>>,
}

pub struct ReportingTask {
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl McpReporter {
    pub fn start() -> (Self, ReportingTask) {
        let url = format!("ws://127.0.0.1:{REPORTING_PORT}/ws/mcp");
        // Isolated process tests must not stop a user's Desktop occupying 26035.
        // Installed release builds always use the product's fixed port.
        #[cfg(debug_assertions)]
        let url = std::env::var("PAB_TEST_MCP_REPORT_URL").unwrap_or(url);
        Self::start_at(url)
    }
    pub fn start_at(url: String) -> (Self, ReportingTask) {
        let (report, receiver) = watch::channel(McpReport::new());
        let (shutdown, stopped) = watch::channel(false);
        let task = tokio::spawn(report_loop(url, receiver, stopped));
        (
            Self {
                report: Arc::new(report),
            },
            ReportingTask { shutdown, task },
        )
    }
    pub fn set_client(&self, name: String, version: String) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        self.report.send_if_modified(|r| {
            if r.client_name.as_deref() == Some(name)
                && r.client_version.as_deref() == Some(&version)
            {
                return false;
            }
            r.client_name = Some(name.to_owned());
            r.client_version = Some(version.clone());
            r.updated_at_unix_ms = now_ms();
            true
        });
    }
    pub fn set_runtime(&self, mut runtime: RuntimeReport) {
        self.report.send_if_modified(|r| {
            // Sample time alone must not cause a new UI event each second.
            if let Some(previous) = &r.runtime {
                runtime.sampled_at_unix_ms = previous.sampled_at_unix_ms;
            }
            if r.runtime.as_ref() == Some(&runtime) {
                return false;
            }
            runtime.sampled_at_unix_ms = now_ms();
            r.runtime = Some(runtime);
            r.updated_at_unix_ms = now_ms();
            true
        });
    }
    pub fn begin_call(&self, tool: &str, arguments: &serde_json::Value) -> ToolCallGuard {
        let id = Uuid::new_v4();
        let call = ToolCallReport {
            id,
            tool: tool.to_owned(),
            started_at_unix_ms: now_ms(),
            finished_at_unix_ms: None,
            device_code: arguments
                .get("device_code")
                .and_then(|v| v.as_str())
                .and_then(|v| {
                    v.chars()
                        .filter(|c| !c.is_whitespace())
                        .collect::<String>()
                        .parse::<pab_protocol::DeviceCode>()
                        .ok()
                })
                .map(|v| v.to_string()),
            device_ref: arguments
                .get("device_ref")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok()),
            task_id: arguments
                .get("task_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            succeeded: None,
            request_id: arguments
                .get("request_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            terminal_session_id: arguments
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
        };
        self.report.send_modify(|r| {
            r.active_calls.push(call);
            r.updated_at_unix_ms = now_ms();
        });
        ToolCallGuard {
            reporter: self.clone(),
            id,
            succeeded: None,
        }
    }
    pub fn attach_runtime(&self, source: crate::runtime::RuntimePresenceSource) -> JoinHandle<()> {
        let reporter = self.clone();
        tokio::spawn(async move {
            let mut events = source.subscribe();
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = interval.tick() => {},
                    event = events.recv() => match event {
                        Ok(event) if matches!(event.kind, crate::RuntimeEventKind::TaskOutput { .. } | crate::RuntimeEventKind::TaskOutputGap { .. }) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        _ => {},
                    }
                }
                match source.snapshot().await {
                    Ok(snapshot) => reporter.set_runtime(snapshot),
                    Err(error) => tracing::debug!(%error, "could not sample MCP runtime status"),
                }
            }
        })
    }
}

pub struct ToolCallGuard {
    reporter: McpReporter,
    id: Uuid,
    succeeded: Option<bool>,
}
impl ToolCallGuard {
    pub fn observe_result(&self, result: &serde_json::Value) {
        self.reporter.report.send_modify(|report| {
            if let Some(call) = report
                .active_calls
                .iter_mut()
                .find(|call| call.id == self.id)
            {
                for path in [
                    "/device_ref",
                    "/task/task_ref/device_ref",
                    "/task_ref/device_ref",
                    "/target/device_ref",
                ] {
                    if let Some(value) = result
                        .pointer(path)
                        .and_then(|value| serde_json::from_value(value.clone()).ok())
                    {
                        call.device_ref = Some(value);
                        break;
                    }
                }
                for path in ["/task/task_ref/task_id", "/task_ref/task_id"] {
                    if let Some(value) = result.pointer(path).and_then(|value| value.as_str()) {
                        call.task_id = Some(value.to_owned());
                        break;
                    }
                }
                for path in [
                    "/task/request_id",
                    "/snapshot/request_id",
                    "/request_id",
                    "/operation_ref/operation_id",
                ] {
                    if let Some(value) = result.pointer(path).and_then(|value| value.as_str()) {
                        call.request_id = Some(value.to_owned());
                        break;
                    }
                }
                if let Some(value) = result.get("session_id").and_then(|value| value.as_str()) {
                    call.terminal_session_id = Some(value.to_owned());
                }
            }
        });
    }
    pub fn finish(&mut self, succeeded: bool) {
        self.succeeded = Some(succeeded);
    }
}
impl Drop for ToolCallGuard {
    fn drop(&mut self) {
        self.reporter.report.send_modify(|r| {
            if let Some(index) = r.active_calls.iter().position(|call| call.id == self.id) {
                let mut call = r.active_calls.remove(index);
                call.finished_at_unix_ms = Some(now_ms());
                call.succeeded = self.succeeded;
                r.recent_calls.insert(0, call);
                r.recent_calls.truncate(20);
                r.updated_at_unix_ms = now_ms();
            }
        });
    }
}
impl ReportingTask {
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let mut task = self.task;
        if tokio::time::timeout(Duration::from_secs(3), &mut task)
            .await
            .is_err()
        {
            task.abort();
        }
    }
}

async fn report_loop(
    url: String,
    mut report: watch::Receiver<McpReport>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        tokio::select! {
            _ = shutdown.changed() => return,
            result = connected_report(&url, &mut report) => {
                if let Err(error) = result { tracing::debug!(%error, "Desktop status reporting disconnected; retrying"); }
            },
        }
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = tokio::time::sleep(RECONNECT_INTERVAL) => {},
        }
    }
}

async fn connected_report(
    url: &str,
    report: &mut watch::Receiver<McpReport>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut socket, _) =
        tokio::time::timeout(Duration::from_secs(3), connect_async(url)).await??;
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    let mut acknowledged = tokio::time::Instant::now();
    loop {
        let value = report.borrow_and_update().clone();
        let encoded = serde_json::to_string(&value)?;
        if encoded.len() > MAX_REPORT_BYTES {
            return Err("MCP status snapshot exceeds the message limit".into());
        }
        tokio::time::timeout(
            Duration::from_secs(3),
            socket.send(Message::Text(encoded.into())),
        )
        .await??;
        loop {
            tokio::select! {
                changed = report.changed() => { changed?; break; },
                _ = heartbeat.tick() => {
                    if acknowledged.elapsed() >= HEARTBEAT_TIMEOUT { return Err("desktop heartbeat timed out".into()); }
                    break;
                },
                message = socket.next() => match message {
                    Some(Ok(Message::Text(text))) if text.as_str() == "{\"type\":\"ack\"}" => acknowledged = tokio::time::Instant::now(),
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                            if value.get("type").and_then(|v| v.as_str()) == Some("account_changed") {
                                if let Some(revision) = value.get("revision").and_then(|v| v.as_u64()) {
                                    pab_agent_core::account::notify_account_change(revision);
                                }
                            }
                        }
                    },
                    Some(Ok(Message::Ping(payload))) => { tokio::time::timeout(Duration::from_secs(3), socket.send(Message::Pong(payload))).await??; },
                    Some(Ok(Message::Close(_))) | None => return Ok(()),
                    Some(Err(error)) => return Err(error.into()),
                    Some(Ok(_)) => {},
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn concurrent_calls_and_cancellation_preserve_identity_and_hide_arguments() {
        let (reporter, task) = McpReporter::start_at("ws://127.0.0.1:1/ws/mcp".to_owned());
        let mut first = reporter.begin_call(
            "pab_run_command",
            &serde_json::json!({"device_code":"516 736 082", "args":["secret-argument"]}),
        );
        let second = reporter.begin_call("pab_list_devices", &serde_json::json!({}));
        assert_eq!(reporter.report.borrow().active_calls.len(), 2);
        assert_ne!(
            reporter.report.borrow().active_calls[0].id,
            reporter.report.borrow().active_calls[1].id
        );
        assert_eq!(
            reporter.report.borrow().active_calls[0]
                .device_code
                .as_deref(),
            Some("516736082")
        );
        assert!(
            !serde_json::to_string(&*reporter.report.borrow())
                .unwrap()
                .contains("secret-argument")
        );
        first.finish(false);
        drop(first);
        drop(second);
        assert!(reporter.report.borrow().active_calls.is_empty());
        assert_eq!(reporter.report.borrow().recent_calls.len(), 2);
        assert_eq!(reporter.report.borrow().recent_calls[0].succeeded, None);
        assert_eq!(
            reporter.report.borrow().recent_calls[1].succeeded,
            Some(false)
        );
        task.shutdown().await;
    }
}
