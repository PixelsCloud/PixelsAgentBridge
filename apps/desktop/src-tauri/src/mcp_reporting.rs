use axum::{
    Json, Router,
    extract::{
        ConnectInfo, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
    routing::get,
};
use pab_bridge::desktop_presence::{
    ConnectedMcp, HEARTBEAT_TIMEOUT, MAX_REPORT_BYTES, McpReport, McpReportingStatus,
    REPORTING_PORT, REPORTING_PROTOCOL_VERSION, now_ms,
};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tauri::Emitter;
use tokio::sync::watch;

struct Slot {
    owner: u64,
    client: ConnectedMcp,
}
struct Inner {
    slots: Mutex<BTreeMap<String, Slot>>,
    status: watch::Sender<McpReportingStatus>,
    next_owner: AtomicU64,
    shutdown: watch::Sender<bool>,
    account_revision: watch::Sender<u64>,
    idle_timeout: Duration,
}

#[derive(Clone)]
pub(crate) struct McpReportingState(Arc<Inner>);
impl Default for McpReportingState {
    fn default() -> Self {
        let (status, _) = watch::channel(McpReportingStatus {
            listen_address: format!("0.0.0.0:{REPORTING_PORT}"),
            ..Default::default()
        });
        let (shutdown, _) = watch::channel(false);
        Self(Arc::new(Inner {
            slots: Mutex::new(BTreeMap::new()),
            status,
            next_owner: AtomicU64::new(1),
            shutdown,
            account_revision: watch::channel(
                pab_agent_core::account::AccountStore::from_env()
                    .and_then(|store| store.read())
                    .map(|state| state.revision)
                    .unwrap_or(0),
            )
            .0,
            idle_timeout: HEARTBEAT_TIMEOUT,
        }))
    }
}

impl McpReportingState {
    pub(crate) fn account_changed(&self, revision: u64) {
        self.0.account_revision.send_replace(revision);
    }
    pub(crate) fn snapshot(&self) -> McpReportingStatus {
        self.0.status.borrow().clone()
    }
    pub(crate) fn stop(&self) {
        let _ = self.0.shutdown.send(true);
    }
    fn publish(&self, slots: &BTreeMap<String, Slot>) {
        self.0.status.send_modify(|status| {
            status.revision += 1;
            status.clients = slots.values().map(|slot| slot.client.clone()).collect();
            status.count = status.clients.len();
        });
    }
    fn upsert(&self, owner: u64, peer: SocketAddr, report: McpReport, registered: bool) -> bool {
        let mut slots = self.0.slots.lock().unwrap_or_else(|e| e.into_inner());
        let key = report.session_id.to_string();
        if registered && slots.get(&key).is_none_or(|slot| slot.owner != owner) {
            return false;
        }
        let now = now_ms();
        let connected = slots
            .get(&key)
            .filter(|slot| slot.owner == owner)
            .map(|slot| slot.client.connected_at_unix_ms)
            .unwrap_or(now);
        slots.insert(
            key,
            Slot {
                owner,
                client: ConnectedMcp {
                    report,
                    peer_address: peer.to_string(),
                    connected_at_unix_ms: connected,
                    last_seen_at_unix_ms: now,
                },
            },
        );
        self.publish(&slots);
        true
    }
    fn remove(&self, owner: u64, session: &str) {
        let mut slots = self.0.slots.lock().unwrap_or_else(|e| e.into_inner());
        if slots.get(session).is_some_and(|slot| slot.owner == owner) {
            slots.remove(session);
            self.publish(&slots);
        }
    }
}

#[tauri::command]
pub(crate) fn mcp_reporting_status(
    state: tauri::State<'_, McpReportingState>,
) -> McpReportingStatus {
    state.snapshot()
}

pub(crate) async fn start(handle: tauri::AppHandle, state: McpReportingState) {
    let mut updates = state.0.status.subscribe();
    let mut stopped = state.0.shutdown.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                _ = stopped.changed() => break,
                changed = updates.changed() => {
                    if changed.is_err() { break; }
                    let status = updates.borrow_and_update().clone();
                    let _ = handle.emit("mcp-reporting-changed", status);
                }
            }
        }
    });
    if let Err(error) = bind(
        state.clone(),
        SocketAddr::from(([0, 0, 0, 0], REPORTING_PORT)),
    )
    .await
    {
        tracing::error!(%error, "could not start Desktop MCP reporting service");
        state.0.status.send_modify(|status| {
            status.revision += 1;
            status.running = false;
            status.error = Some(error.to_string());
        });
    }
}

async fn bind(state: McpReportingState, address: SocketAddr) -> std::io::Result<SocketAddr> {
    let listener = tokio::net::TcpListener::bind(address).await?;
    let address = listener.local_addr()?;
    let router = Router::new()
        .route("/health", get(|| async { Json(serde_json::json!({ "status": "ok", "protocolVersion": REPORTING_PROTOCOL_VERSION })) }))
        .route("/api/mcp", get(http_status))
        .route("/ws/mcp", get(upgrade))
        .with_state(state.clone());
    state.0.status.send_modify(|status| {
        status.revision += 1;
        status.running = true;
        status.error = None;
        status.listen_address = address.to_string();
    });
    let mut stopped = state.0.shutdown.subscribe();
    tauri::async_runtime::spawn(async move {
        let shutdown = async move {
            if !*stopped.borrow() {
                let _ = stopped.changed().await;
            }
        };
        if let Err(error) = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await
        {
            tracing::error!(%error, "Desktop MCP reporting service failed");
            state.0.status.send_modify(|status| {
                status.revision += 1;
                status.running = false;
                status.error = Some(error.to_string());
            });
        } else {
            state.0.status.send_modify(|status| {
                status.revision += 1;
                status.running = false;
                status.clients.clear();
                status.count = 0;
            });
        }
    });
    tracing::info!(%address, "Desktop MCP HTTP/WebSocket service started");
    Ok(address)
}

async fn http_status(State(state): State<McpReportingState>) -> Json<McpReportingStatus> {
    Json(state.snapshot())
}
async fn upgrade(
    State(state): State<McpReportingState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.max_message_size(MAX_REPORT_BYTES)
        .max_frame_size(MAX_REPORT_BYTES)
        .on_upgrade(move |socket| connection(socket, state, peer))
}

async fn send_account_revision(socket: &mut WebSocket, revision: u64) -> bool {
    let text = serde_json::json!({"type":"account_changed","revision":revision}).to_string();
    matches!(
        tokio::time::timeout(
            Duration::from_secs(3),
            socket.send(Message::Text(text.into()))
        )
        .await,
        Ok(Ok(()))
    )
}

async fn connection(mut socket: WebSocket, state: McpReportingState, peer: SocketAddr) {
    let owner = state.0.next_owner.fetch_add(1, Ordering::Relaxed);
    let mut session: Option<String> = None;
    let mut shutdown = state.0.shutdown.subscribe();
    let mut account = state.0.account_revision.subscribe();
    let initial_revision = *account.borrow_and_update();
    if !send_account_revision(&mut socket, initial_revision).await {
        return;
    }
    loop {
        if *shutdown.borrow() {
            break;
        }
        let message = tokio::select! {
            _ = shutdown.changed() => break,
            changed = account.changed() => {
                if changed.is_err() { break; }
                let revision = *account.borrow_and_update();
                if !send_account_revision(&mut socket, revision).await { break; }
                continue;
            },
            result = tokio::time::timeout(state.0.idle_timeout, socket.recv()) => match result { Ok(Some(Ok(message))) => message, _ => break },
        };
        match message {
            Message::Text(text) => {
                let Ok(report) = serde_json::from_str::<McpReport>(&text) else {
                    break;
                };
                if !report.valid()
                    || session
                        .as_ref()
                        .is_some_and(|id| id != &report.session_id.to_string())
                {
                    break;
                }
                if !state.upsert(owner, peer, report.clone(), session.is_some()) {
                    break;
                }
                session = Some(report.session_id.to_string());
                if !matches!(
                    tokio::time::timeout(
                        Duration::from_secs(3),
                        socket.send(Message::Text("{\"type\":\"ack\"}".into()))
                    )
                    .await,
                    Ok(Ok(()))
                ) {
                    break;
                }
            }
            Message::Ping(data) => {
                if !matches!(
                    tokio::time::timeout(Duration::from_secs(3), socket.send(Message::Pong(data)))
                        .await,
                    Ok(Ok(()))
                ) {
                    break;
                }
            }
            Message::Close(_) => break,
            Message::Pong(_) => {}
            Message::Binary(_) => break,
        }
    }
    if let Some(session) = session {
        state.remove(owner, &session);
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.send(Message::Close(None))).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use pab_bridge::desktop_presence::McpReporter;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_tungstenite::{connect_async, tungstenite::Message as ClientMessage};

    async fn receive_ack(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let message = socket.next().await.unwrap().unwrap();
                if let ClientMessage::Text(text) = message {
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if value["type"] == "ack" {
                        break;
                    }
                    assert_eq!(value["type"], "account_changed");
                    assert_eq!(value.as_object().unwrap().len(), 2);
                }
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn account_changes_broadcast_only_revisions_and_reconnect_gets_latest() {
        let state = McpReportingState::default();
        state.account_changed(10);
        let address = bind(state.clone(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let url = format!("ws://{address}/ws/mcp");
        let (mut first, _) = connect_async(&url).await.unwrap();
        let (mut second, _) = connect_async(&url).await.unwrap();
        for revision in [10, 12] {
            if revision == 12 {
                state.account_changed(revision);
            }
            for socket in [&mut first, &mut second] {
                let text = tokio::time::timeout(Duration::from_secs(3), socket.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
                    .into_text()
                    .unwrap();
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&text).unwrap(),
                    serde_json::json!({"type":"account_changed", "revision":revision})
                );
            }
        }
        drop(first);
        let (mut restarted, _) = connect_async(&url).await.unwrap();
        let text = restarted
            .next()
            .await
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap()["revision"],
            12
        );
        state.stop();
    }

    async fn wait_for(state: &McpReportingState, predicate: impl Fn(&McpReportingStatus) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(7);
        loop {
            if predicate(&state.snapshot()) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "status condition timed out: {:?}",
                state.snapshot()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn http(address: SocketAddr, path: &str) -> serde_json::Value {
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        socket
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), socket.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        let response = String::from_utf8(response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
    }

    #[tokio::test]
    async fn http_and_websocket_share_the_listener_and_track_independent_mcps() {
        let state = McpReportingState::default();
        let address = bind(state.clone(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let url = format!("ws://{address}/ws/mcp");
        let (first, first_task) = McpReporter::start_at(url.clone());
        let (_second, second_task) = McpReporter::start_at(url);
        first.set_client("test-agent".to_owned(), "1".to_owned());
        wait_for(&state, |status| status.count == 2).await;
        assert_eq!(http(address, "/health").await["status"], "ok");
        assert_eq!(http(address, "/api/mcp").await["count"], 2);
        let mut call = first.begin_call(
            "pab_connect",
            &serde_json::json!({"device_code":"516 736 082", "password":"must-never-be-reported"}),
        );
        wait_for(&state, |status| {
            status
                .clients
                .iter()
                .any(|client| client.report.active_calls.len() == 1)
        })
        .await;
        let result = http(address, "/api/mcp").await;
        assert!(!result.to_string().contains("must-never-be-reported"));
        assert!(result.to_string().contains("516736082"));
        call.finish(true);
        drop(call);
        wait_for(&state, |status| {
            status.clients.iter().any(|client| {
                client
                    .report
                    .recent_calls
                    .first()
                    .is_some_and(|call| call.succeeded == Some(true))
            })
        })
        .await;
        first_task.shutdown().await;
        wait_for(&state, |status| status.count == 1).await;
        second_task.shutdown().await;
        wait_for(&state, |status| status.count == 0).await;
        state.stop();
    }

    #[tokio::test]
    async fn old_connection_cannot_overwrite_or_remove_a_reconnected_session() {
        let state = McpReportingState::default();
        let address = bind(state.clone(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let url = format!("ws://{address}/ws/mcp");
        let report = McpReport::new();
        let (mut old, _) = connect_async(&url).await.unwrap();
        old.send(ClientMessage::Text(
            serde_json::to_string(&report).unwrap().into(),
        ))
        .await
        .unwrap();
        receive_ack(&mut old).await;
        let (mut new, _) = connect_async(&url).await.unwrap();
        let mut replacement = report.clone();
        replacement.client_name = Some("new".to_owned());
        new.send(ClientMessage::Text(
            serde_json::to_string(&replacement).unwrap().into(),
        ))
        .await
        .unwrap();
        receive_ack(&mut new).await;
        assert_eq!(state.snapshot().count, 1);
        old.send(ClientMessage::Text(
            serde_json::to_string(&report).unwrap().into(),
        ))
        .await
        .unwrap();
        let _ = old.next().await;
        assert_eq!(
            state.snapshot().clients[0].report.client_name.as_deref(),
            Some("new")
        );
        drop(old);
        assert_eq!(state.snapshot().count, 1);
        drop(new);
        wait_for(&state, |status| status.count == 0).await;
        state.stop();
    }

    #[tokio::test]
    async fn malformed_messages_and_missing_heartbeats_remove_registered_clients() {
        let mut state = McpReportingState::default();
        Arc::get_mut(&mut state.0).unwrap().idle_timeout = Duration::from_millis(150);
        let address = bind(state.clone(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let url = format!("ws://{address}/ws/mcp");
        let (mut socket, _) = connect_async(&url).await.unwrap();
        socket
            .send(ClientMessage::Text(
                serde_json::to_string(&McpReport::new()).unwrap().into(),
            ))
            .await
            .unwrap();
        socket.next().await.unwrap().unwrap();
        wait_for(&state, |status| status.count == 1).await;
        wait_for(&state, |status| status.count == 0).await;
        let (mut invalid, _) = connect_async(&url).await.unwrap();
        invalid
            .send(ClientMessage::Text("{invalid}".into()))
            .await
            .unwrap();
        let _ = invalid.next().await;
        assert_eq!(state.snapshot().count, 0);
        let (mut wrong_version, _) = connect_async(&url).await.unwrap();
        let mut report = McpReport::new();
        report.protocol_version = 99;
        wrong_version
            .send(ClientMessage::Text(
                serde_json::to_string(&report).unwrap().into(),
            ))
            .await
            .unwrap();
        let _ = wrong_version.next().await;
        assert_eq!(http(address, "/health").await["status"], "ok");
        state.stop();
    }

    #[tokio::test]
    async fn mcp_connects_when_desktop_starts_later_and_recovers_after_restart() {
        let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = reservation.local_addr().unwrap();
        drop(reservation);
        let (reporter, task) = McpReporter::start_at(format!("ws://{address}/ws/mcp"));
        reporter.set_client("late-start".to_owned(), "1".to_owned());
        tokio::time::sleep(Duration::from_millis(100)).await;
        let first = McpReportingState::default();
        bind(first.clone(), address).await.unwrap();
        wait_for(&first, |status| status.count == 1).await;
        let session = first.snapshot().clients[0].report.session_id;
        first.stop();
        wait_for(&first, |status| !status.running).await;
        let restarted = McpReportingState::default();
        bind(restarted.clone(), address).await.unwrap();
        wait_for(&restarted, |status| status.count == 1).await;
        assert_eq!(restarted.snapshot().clients[0].report.session_id, session);
        task.shutdown().await;
        restarted.stop();
    }

    #[tokio::test]
    async fn occupied_port_fails_without_changing_the_listen_port() {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let state = McpReportingState::default();
        assert!(
            bind(state.clone(), occupied.local_addr().unwrap())
                .await
                .is_err()
        );
        assert!(!state.snapshot().running);
        assert_eq!(state.snapshot().listen_address, "0.0.0.0:26035");
    }

    #[tokio::test]
    #[ignore = "build pab-mcp and set PAB_MCP_SMOKE_EXE before running this real-process smoke test"]
    async fn real_mcp_stdio_processes_register_before_tools_and_clean_up_on_exit() {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let executable = std::env::var_os("PAB_MCP_SMOKE_EXE")
            .expect("PAB_MCP_SMOKE_EXE must point to the newly built pab-mcp executable");
        let directory = tempfile::tempdir().unwrap();
        let state = McpReportingState::default();
        let address = bind(state.clone(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let mut processes = Vec::new();
        let mut inputs = Vec::new();
        let account_url = std::env::var("PAB_MCP_ACCOUNT_TEST_URL").ok();
        let account_ca = std::env::var_os("PAB_MCP_ACCOUNT_TEST_CA");
        let process_count = std::env::var("PAB_MCP_SMOKE_COUNT")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(2)
            .clamp(2, 10);
        for index in 0..process_count {
            let mut command = tokio::process::Command::new(&executable);
            command
                .env("PAB_TEST_MCP_REPORT_URL", format!("ws://{address}/ws/mcp"))
                .env("PAB_DATA_DIR", directory.path())
                .env(
                    "PAB_BRIDGE_DATABASE",
                    directory.path().join("bridge.sqlite3"),
                )
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            if let Some(url) = &account_url {
                command
                    .env("PAB_CONTROL_URL", url)
                    .env("PAB_MCP_GUEST", "1")
                    .env("PAB_RELAY_URLS", "https://localhost:38443/");
                if let Some(ca) = &account_ca {
                    command.env("PAB_CONTROL_CA_CERT", ca);
                }
            }
            #[cfg(windows)]
            command.creation_flags(0x08000000);
            let mut process = command.spawn().unwrap();
            let mut input = process.stdin.take().unwrap();
            let mut output = BufReader::new(process.stdout.take().unwrap()).lines();
            input.write_all(format!("{}\n", serde_json::json!({
                "jsonrpc":"2.0", "id":1, "method":"initialize", "params": {
                    "protocolVersion":"2025-11-25", "capabilities":{}, "clientInfo":{"name":format!("reporting-smoke-{index}"),"version":"1"}
                }
            })).as_bytes()).await.unwrap();
            let response = tokio::time::timeout(Duration::from_secs(5), output.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let response: serde_json::Value = serde_json::from_str(&response).unwrap();
            assert!(
                response.get("result").is_some(),
                "initialize failed: {response}"
            );
            input
                .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
                .await
                .unwrap();
            processes.push((process, output));
            inputs.push(input);
        }
        wait_for(&state, |status| {
            status.count == process_count
                && status
                    .clients
                    .iter()
                    .all(|client| client.report.client_name.is_some())
        })
        .await;
        assert!(
            state
                .snapshot()
                .clients
                .iter()
                .all(|client| client.report.runtime.is_none())
        );
        assert_ne!(
            state.snapshot().clients[0].report.process_id,
            state.snapshot().clients[1].report.process_id
        );
        inputs[0].write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"pab_list_devices\",\"arguments\":{}}}\n").await.unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), processes[0].1.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["id"], 2);
        assert!(response.get("result").is_some());
        assert_ne!(response["result"]["isError"], true);
        wait_for(&state, |status| {
            status.clients.iter().any(|client| {
                client.report.recent_calls.first().is_some_and(|call| {
                    call.tool == "pab_list_devices" && call.succeeded == Some(true)
                })
            })
        })
        .await;
        assert_eq!(
            http(
                SocketAddr::from(([127, 0, 0, 1], address.port())),
                "/api/mcp"
            )
            .await["count"],
            process_count
        );
        if let Some(url) = account_url {
            use pab_agent_core::account::{AccountClient, AccountStore};
            let ca = account_ca.map(std::fs::read).transpose().unwrap();
            let client = AccountClient::new(&url, ca.as_deref()).unwrap();
            let store = AccountStore::new(directory.path(), &url).unwrap();
            let sessions: Vec<_> = state
                .snapshot()
                .clients
                .iter()
                .map(|c| c.report.session_id)
                .collect();
            // Resolving an intentionally absent device initializes each real runtime.
            // Device I/O and task preservation are covered by control_tls separately.
            for index in 0..process_count {
                inputs[index].write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"pab_connect\",\"arguments\":{\"device_code\":\"111222333\",\"wait_ms\":5000}}}\n").await.unwrap();
                let response =
                    tokio::time::timeout(Duration::from_secs(15), processes[index].1.next_line())
                        .await
                        .unwrap()
                        .unwrap()
                        .unwrap();
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&response).unwrap()["id"],
                    3
                );
            }
            wait_for(&state, |s| {
                s.count == process_count
                    && s.clients.iter().all(|c| {
                        c.report
                            .runtime
                            .as_ref()
                            .and_then(|r| r.account.as_ref())
                            .is_some_and(|a| a.server_revision == Some(0))
                    })
            })
            .await;
            for label in ["first", "second"] {
                let session = client
                    .login(
                        &format!("mcp-{label}-{}", pab_protocol::RequestId::new()),
                        zeroize::Zeroizing::new("test password long enough".into()),
                        true,
                    )
                    .await
                    .unwrap();
                let user_id = session.user.id;
                let account = store.login(session).unwrap();
                if label == "first" {
                    state.account_changed(account.revision);
                }
                // The second login deliberately loses its IPC notification; polling repairs it.
                wait_for(&state, |s| {
                    s.count == process_count
                        && s.clients.iter().all(|c| {
                            c.report
                                .runtime
                                .as_ref()
                                .and_then(|r| r.account.as_ref())
                                .is_some_and(|a| {
                                    a.local_revision == account.revision
                                        && a.server_revision == Some(account.revision)
                                        && a.user.as_ref().is_some_and(|u| u.user_id == user_id)
                                })
                        })
                })
                .await;
                assert!(
                    state
                        .snapshot()
                        .clients
                        .iter()
                        .all(|c| sessions.contains(&c.report.session_id)
                            && c.report.runtime.as_ref().unwrap().identity == "guest")
                );
                let guest = store.logout().unwrap();
                state.account_changed(guest.revision);
                client.flush_logouts(&store).await.unwrap();
                wait_for(&state, |s| {
                    s.count == process_count
                        && s.clients.iter().all(|c| {
                            c.report
                                .runtime
                                .as_ref()
                                .and_then(|r| r.account.as_ref())
                                .is_some_and(|a| {
                                    a.server_revision == Some(guest.revision) && a.user.is_none()
                                })
                        })
                })
                .await;
            }
        }
        processes[0].0.kill().await.unwrap();
        wait_for(&state, |status| status.count == process_count - 1).await;
        inputs.clear();
        for (process, _) in processes.iter_mut().skip(1) {
            let status = tokio::time::timeout(Duration::from_secs(15), process.wait())
                .await
                .unwrap()
                .unwrap();
            assert!(status.success(), "MCP did not shut down cleanly: {status}");
        }
        wait_for(&state, |status| status.count == 0).await;
        state.stop();
    }
}
