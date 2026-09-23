use std::{path::PathBuf, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
};
use pab_agent_core::{begin_device_claim, begin_personal_device_claim, tls_connector};
use pab_bridge::{BridgeRuntime, MemoryDevicePasswordProvider};
use pab_protocol::{DeviceCode, OutputStream, RequestId, TaskId, TaskRef};
use rand_core::{OsRng, RngCore};
use serde::Deserialize;
use serde_json::{Value, json};
use zeroize::Zeroizing;

#[derive(Clone)]
struct UiState {
    runtime: Arc<BridgeRuntime>,
    passwords: Arc<MemoryDevicePasswordProvider>,
    control_url: String,
    control_ca_cert: Option<PathBuf>,
    token: String,
}

#[derive(Deserialize)]
struct AuthQuery {
    token: String,
}

#[derive(Deserialize)]
struct TaskQuery {
    token: String,
    device_code: String,
    task_id: String,
}

#[derive(Deserialize)]
struct OutputQuery {
    token: String,
    device_code: String,
    task_id: String,
    stream: String,
    offset: Option<u64>,
}

#[derive(Deserialize)]
struct ConnectRequest {
    device_code: String,
}

#[derive(Deserialize)]
struct PasswordRequest {
    device_code: String,
    password: String,
}

#[derive(Deserialize)]
struct ClaimRequest {
    device_code: String,
    username: String,
    password: String,
    owner_tenant_id: Option<String>,
}

#[derive(Deserialize)]
struct CommandRequest {
    device_code: String,
    program: String,
    args: Vec<String>,
    cwd: Option<String>,
}

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

pub async fn serve(
    runtime: Arc<BridgeRuntime>,
    passwords: Arc<MemoryDevicePasswordProvider>,
    control_url: String,
    control_ca_cert: Option<PathBuf>,
    port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut token_bytes = [0u8; 16];
    OsRng.fill_bytes(&mut token_bytes);
    let token = token_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let state = UiState {
        runtime,
        passwords,
        control_url,
        control_ca_cert,
        token,
    };
    let app = Router::new()
        .route("/", get(index))
        .route("/api/devices", get(devices))
        .route("/api/connect", post(connect))
        .route("/api/password", post(password))
        .route("/api/claim", post(claim))
        .route("/api/command", post(command))
        .route("/api/task", get(task))
        .route("/api/output", get(output))
        .route("/api/events", get(events))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let address = listener.local_addr()?;
    eprintln!(
        "pab-ui: http://127.0.0.1:{address_port}/?token={token}",
        address_port = address.port(),
        token = state.token
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index(State(state): State<UiState>, Query(query): Query<AuthQuery>) -> impl IntoResponse {
    if query.token != state.token {
        return (StatusCode::FORBIDDEN, Html("Access denied"));
    }
    (StatusCode::OK, Html(include_str!("ui.html")))
}

async fn devices(State(state): State<UiState>, Query(query): Query<AuthQuery>) -> ApiResult {
    authorize(&state, &query.token)?;
    let devices = state.runtime.list_devices().await.map_err(internal_error)?;
    Ok(Json(json!({ "devices": devices })))
}

async fn connect(
    State(state): State<UiState>,
    Query(query): Query<AuthQuery>,
    Json(request): Json<ConnectRequest>,
) -> ApiResult {
    authorize(&state, &query.token)?;
    let device_ref = resolve(&state.runtime, &request.device_code).await?;
    let target = state
        .runtime
        .current_environment(device_ref)
        .await
        .map_err(internal_error)?;
    Ok(Json(json!({
        "device_ref": device_ref,
        "target": target,
        "os_reminder": target.compact_reminder()
    })))
}

async fn password(
    State(state): State<UiState>,
    Query(query): Query<AuthQuery>,
    Json(request): Json<PasswordRequest>,
) -> ApiResult {
    authorize(&state, &query.token)?;
    let device_ref = resolve(&state.runtime, &request.device_code).await?;
    state
        .passwords
        .set_password(device_ref.device_id, request.password)
        .map_err(internal_error)?;
    Ok(Json(json!({ "saved": true })))
}

async fn claim(
    State(state): State<UiState>,
    Query(query): Query<AuthQuery>,
    Json(request): Json<ClaimRequest>,
) -> ApiResult {
    authorize(&state, &query.token)?;
    let code: DeviceCode = request
        .device_code
        .parse()
        .map_err(|_| bad_request("invalid 9-digit device code"))?;
    if request.username.trim().is_empty() || request.password.is_empty() {
        return Err(bad_request("account and password are required"));
    }
    let ca = state
        .control_ca_cert
        .as_ref()
        .map(std::fs::read)
        .transpose()
        .map_err(internal_error)?;
    let connector = tls_connector(ca.as_deref()).map_err(internal_error)?;
    let password = Zeroizing::new(request.password);
    let timeout = Duration::from_secs(10);
    let (claim_id, owner_tenant_id) = match request
        .owner_tenant_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(owner_tenant_id) => {
            let owner_tenant_id = owner_tenant_id
                .parse()
                .map_err(|_| bad_request("invalid Team ID"))?;
            let claim_id = begin_device_claim(
                &state.control_url,
                request.username,
                password,
                code,
                owner_tenant_id,
                connector,
                timeout,
            )
            .await
            .map_err(internal_error)?;
            (claim_id, owner_tenant_id)
        }
        None => begin_personal_device_claim(
            &state.control_url,
            request.username,
            password,
            code,
            connector,
            timeout,
        )
        .await
        .map_err(internal_error)?,
    };
    Ok(Json(json!({
        "claim_id": claim_id,
        "owner_tenant_id": owner_tenant_id
    })))
}

async fn command(
    State(state): State<UiState>,
    Query(query): Query<AuthQuery>,
    Json(request): Json<CommandRequest>,
) -> ApiResult {
    authorize(&state, &query.token)?;
    if request.program.trim().is_empty() {
        return Err(bad_request("program is required"));
    }
    let device_ref = resolve(&state.runtime, &request.device_code).await?;
    let snapshot = state
        .runtime
        .submit_command(
            device_ref,
            RequestId::new(),
            request.program,
            request.args,
            request.cwd,
        )
        .await
        .map_err(internal_error)?;
    Ok(Json(json!({ "task": snapshot })))
}

async fn task(State(state): State<UiState>, Query(query): Query<TaskQuery>) -> ApiResult {
    authorize(&state, &query.token)?;
    let task_ref = task_ref(&state.runtime, &query.device_code, &query.task_id).await?;
    let record = state.runtime.task(task_ref).await.map_err(internal_error)?;
    Ok(Json(json!({
        "task_ref": task_ref,
        "snapshot": record.snapshot,
        "complete": record.is_complete(),
        "last_event_seq": record.last_event_seq,
        "stdout": record.stdout,
        "stderr": record.stderr
    })))
}

async fn output(State(state): State<UiState>, Query(query): Query<OutputQuery>) -> ApiResult {
    authorize(&state, &query.token)?;
    let task_ref = task_ref(&state.runtime, &query.device_code, &query.task_id).await?;
    let stream = match query.stream.as_str() {
        "stdout" => OutputStream::Stdout,
        "stderr" => OutputStream::Stderr,
        _ => return Err(bad_request("stream must be stdout or stderr")),
    };
    let offset = query.offset.unwrap_or(0);
    let (chunk, range) = state
        .runtime
        .read_output(task_ref, stream, offset, 64 * 1024)
        .await
        .map_err(internal_error)?;
    Ok(Json(json!({
        "text": String::from_utf8_lossy(&chunk.bytes),
        "next_offset": offset + chunk.bytes.len() as u64,
        "range": range
    })))
}

async fn events(
    State(state): State<UiState>,
    Query(query): Query<AuthQuery>,
    websocket: WebSocketUpgrade,
) -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    authorize(&state, &query.token)?;
    let receiver = state.runtime.subscribe();
    Ok(websocket.on_upgrade(move |socket| stream_events(socket, receiver)))
}

async fn stream_events(
    mut socket: WebSocket,
    mut receiver: tokio::sync::broadcast::Receiver<pab_bridge::RuntimeEvent>,
) {
    loop {
        match receiver.recv().await {
            Ok(event) => {
                let Ok(encoded) = serde_json::to_string(&event) else {
                    break;
                };
                if socket.send(Message::Text(encoded.into())).await.is_err() {
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn resolve(
    runtime: &BridgeRuntime,
    code: &str,
) -> Result<pab_protocol::DeviceRef, (StatusCode, Json<Value>)> {
    let code: DeviceCode = code
        .parse()
        .map_err(|_| bad_request("invalid 9-digit device code"))?;
    runtime
        .resolve_device_code(code)
        .await
        .map_err(internal_error)
}

async fn task_ref(
    runtime: &BridgeRuntime,
    code: &str,
    task_id: &str,
) -> Result<TaskRef, (StatusCode, Json<Value>)> {
    let device_ref = resolve(runtime, code).await?;
    let task_id: TaskId = task_id
        .parse()
        .map_err(|_| bad_request("invalid task ID"))?;
    Ok(TaskRef {
        device_ref,
        task_id,
    })
}

fn authorize(state: &UiState, token: &str) -> Result<(), (StatusCode, Json<Value>)> {
    if token == state.token {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "access denied" })),
        ))
    }
}

fn bad_request(message: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message })))
}

fn internal_error(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({ "error": error.to_string() })),
    )
}
