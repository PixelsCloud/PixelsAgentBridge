use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use pab_agent_core::{DataPaths, DataScope, ensure_data_parent, restrict_private_file};
use pab_protocol::{ClaimId, DesktopInputEvent};
use pab_protocol::{MAX_SCREENSHOT_BYTES, MAX_WINDOW_ENTRIES, MAX_WINDOW_TITLE_BYTES, WindowEntry};
use rand_core::{OsRng, RngCore};
use serde_json::{Value, json};
use sha2::Sha256;
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    sync::{Semaphore, mpsc, oneshot},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, accept_async, connect_async, tungstenite::Message,
};

use crate::{approve_claim, device_status::read_device_status_from_service};

const TOKEN_FILE: &str = "local-access.key";
const DEFAULT_PORT: u16 = 7843;
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
type HmacSha256 = Hmac<Sha256>;
pub type LocalSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct WindowProvider {
    id: u64,
    sender: mpsc::Sender<HelperRequest>,
}

enum HelperRequest {
    Windows(oneshot::Sender<Result<Vec<WindowEntry>, LocalIpcError>>),
    Screenshot(oneshot::Sender<Result<Vec<u8>, LocalIpcError>>),
    DesktopInput(
        DesktopInputEvent,
        oneshot::Sender<Result<(), LocalIpcError>>,
    ),
}

enum PendingRequest {
    Windows(oneshot::Sender<Result<Vec<WindowEntry>, LocalIpcError>>),
    Screenshot(oneshot::Sender<Result<Vec<u8>, LocalIpcError>>),
    DesktopInput(oneshot::Sender<Result<(), LocalIpcError>>),
}

static WINDOW_PROVIDERS: OnceLock<Mutex<Vec<WindowProvider>>> = OnceLock::new();
static NEXT_PROVIDER_ID: AtomicU64 = AtomicU64::new(1);

fn window_providers() -> &'static Mutex<Vec<WindowProvider>> {
    WINDOW_PROVIDERS.get_or_init(|| Mutex::new(Vec::new()))
}

pub(crate) async fn request_window_list() -> Result<Vec<WindowEntry>, LocalIpcError> {
    let sender = window_providers()
        .lock()
        .map_err(|_| LocalIpcError::Protocol)?
        .last()
        .map(|provider| provider.sender.clone());
    let Some(sender) = sender else {
        tracing::warn!("window listing requested without a registered interactive helper");
        return Err(LocalIpcError::WindowHelperUnavailable);
    };
    let (reply, receiver) = oneshot::channel();
    sender
        .send(HelperRequest::Windows(reply))
        .await
        .map_err(|_| LocalIpcError::WindowHelperUnavailable)?;
    tokio::time::timeout(Duration::from_secs(8), receiver)
        .await
        .map_err(|_| LocalIpcError::Timeout)?
        .map_err(|_| LocalIpcError::WindowHelperUnavailable)?
}

pub(crate) async fn request_screenshot() -> Result<Vec<u8>, LocalIpcError> {
    let sender = window_providers()
        .lock()
        .map_err(|_| LocalIpcError::Protocol)?
        .last()
        .map(|provider| provider.sender.clone());
    let Some(sender) = sender else {
        return Err(LocalIpcError::WindowHelperUnavailable);
    };
    let (reply, receiver) = oneshot::channel();
    sender
        .send(HelperRequest::Screenshot(reply))
        .await
        .map_err(|_| LocalIpcError::WindowHelperUnavailable)?;
    tokio::time::timeout(Duration::from_secs(15), receiver)
        .await
        .map_err(|_| LocalIpcError::Timeout)?
        .map_err(|_| LocalIpcError::WindowHelperUnavailable)?
}

pub(crate) async fn request_desktop_input(event: DesktopInputEvent) -> Result<(), LocalIpcError> {
    let sender = window_providers()
        .lock()
        .map_err(|_| LocalIpcError::Protocol)?
        .last()
        .map(|provider| provider.sender.clone())
        .ok_or(LocalIpcError::WindowHelperUnavailable)?;
    let (reply, receiver) = oneshot::channel();
    sender
        .send(HelperRequest::DesktopInput(event, reply))
        .await
        .map_err(|_| LocalIpcError::WindowHelperUnavailable)?;
    tokio::time::timeout(Duration::from_secs(5), receiver)
        .await
        .map_err(|_| LocalIpcError::Timeout)?
        .map_err(|_| LocalIpcError::WindowHelperUnavailable)?
}

#[derive(Debug)]
pub enum LocalEvent {
    Status(crate::DeviceStatus),
    StatusUnavailable(String),
    ListWindows,
    CaptureScreenshot,
    DesktopInput(DesktopInputEvent),
}

pub async fn register_window_helper(socket: &mut LocalSocket) -> Result<(), LocalIpcError> {
    send_json(socket, &json!({"type":"register_window_helper"})).await?;
    tokio::time::timeout(AUTH_TIMEOUT, async {
        loop {
            let frame = receive_json(socket).await?;
            if frame["type"] == "window_helper_registered" {
                return Ok(());
            }
            if frame["type"] != "status" && frame["type"] != "error" {
                return Err(LocalIpcError::Protocol);
            }
        }
    })
    .await
    .map_err(|_| LocalIpcError::Timeout)?
}

pub async fn next_local_event(socket: &mut LocalSocket) -> Result<LocalEvent, LocalIpcError> {
    loop {
        let frame = receive_json(socket).await?;
        match frame["type"].as_str() {
            Some("status") => {
                return Ok(LocalEvent::Status(serde_json::from_value(
                    frame["status"].clone(),
                )?));
            }
            Some("list_windows") => return Ok(LocalEvent::ListWindows),
            Some("capture_screenshot") => return Ok(LocalEvent::CaptureScreenshot),
            Some("desktop_input") => {
                return Ok(LocalEvent::DesktopInput(serde_json::from_value(
                    frame["event"].clone(),
                )?));
            }
            Some("error") => {
                return Ok(LocalEvent::StatusUnavailable(
                    frame["message"]
                        .as_str()
                        .unwrap_or("local service error")
                        .to_owned(),
                ));
            }
            _ => return Err(LocalIpcError::Protocol),
        }
    }
}

pub async fn reply_window_list(
    socket: &mut LocalSocket,
    entries: &[WindowEntry],
) -> Result<(), LocalIpcError> {
    send_json(socket, &json!({"type":"window_list","entries":entries})).await
}

pub async fn reply_screenshot(socket: &mut LocalSocket, bytes: &[u8]) -> Result<(), LocalIpcError> {
    validate_png(bytes)?;
    socket.send(Message::Binary(bytes.to_vec().into())).await?;
    Ok(())
}

pub async fn reply_screenshot_error(
    socket: &mut LocalSocket,
    message: &str,
) -> Result<(), LocalIpcError> {
    send_json(
        socket,
        &json!({"type":"screenshot_error","message":message}),
    )
    .await
}

pub async fn reply_desktop_input(
    socket: &mut LocalSocket,
    result: Result<(), &str>,
) -> Result<(), LocalIpcError> {
    match result {
        Ok(()) => send_json(socket, &json!({"type":"desktop_input_result","ok":true})).await,
        Err(message) => {
            send_json(
                socket,
                &json!({"type":"desktop_input_result","ok":false,"message":message}),
            )
            .await
        }
    }
}

pub(crate) fn validate_png(bytes: &[u8]) -> Result<(u32, u32), LocalIpcError> {
    if bytes.len() < 33
        || bytes.len() > MAX_SCREENSHOT_BYTES
        || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
        || &bytes[12..16] != b"IHDR"
    {
        return Err(LocalIpcError::InvalidScreenshot);
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0
        || height == 0
        || u64::from(width) * u64::from(height) > pab_protocol::MAX_SCREENSHOT_PIXELS
    {
        return Err(LocalIpcError::InvalidScreenshot);
    }
    Ok((width, height))
}

pub fn local_port() -> Result<u16, LocalIpcError> {
    match env::var("PAB_LOCAL_IPC_PORT") {
        Ok(value) => value.parse().map_err(|_| LocalIpcError::InvalidPort),
        Err(env::VarError::NotPresent) => Ok(DEFAULT_PORT),
        Err(_) => Err(LocalIpcError::InvalidPort),
    }
}

pub fn user_token_path() -> Result<PathBuf, LocalIpcError> {
    Ok(DataPaths::for_scope(DataScope::User)?
        .root()
        .join(TOKEN_FILE))
}

pub fn issue_local_access(destination: &Path) -> Result<(), LocalIpcError> {
    let paths = DataPaths::for_scope(DataScope::Machine)?;
    let token = ensure_machine_token(paths.root())?;
    ensure_data_parent(destination)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(destination)?;
    file.write_all(hex::encode(token).as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    restrict_private_file(destination)?;
    Ok(())
}

fn ensure_machine_token(root: &Path) -> Result<[u8; 32], LocalIpcError> {
    let path = root.join(TOKEN_FILE);
    match read_token(&path) {
        Ok(token) => return Ok(token),
        Err(LocalIpcError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    ensure_data_parent(&path)?;
    let mut token = [0u8; 32];
    OsRng.fill_bytes(&mut token);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            file.write_all(hex::encode(token).as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            restrict_private_file(&path)?;
            Ok(token)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => read_token(&path),
        Err(error) => Err(error.into()),
    }
}

fn read_token(path: &Path) -> Result<[u8; 32], LocalIpcError> {
    let encoded = fs::read_to_string(path)?;
    let mut token = [0u8; 32];
    hex::decode_to_slice(encoded.trim(), &mut token).map_err(|_| LocalIpcError::InvalidToken)?;
    Ok(token)
}

fn proof(token: &[u8; 32], role: &[u8], nonce: &[u8; 32]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(token).expect("HMAC accepts a 32-byte key");
    mac.update(role);
    mac.update(nonce);
    mac.finalize().into_bytes().into()
}

fn verify_proof(
    token: &[u8; 32],
    role: &[u8],
    nonce: &[u8; 32],
    encoded: &str,
) -> Result<(), LocalIpcError> {
    let decoded = hex::decode(encoded).map_err(|_| LocalIpcError::Authentication)?;
    let mut mac = HmacSha256::new_from_slice(token).expect("HMAC accepts a 32-byte key");
    mac.update(role);
    mac.update(nonce);
    mac.verify_slice(&decoded)
        .map_err(|_| LocalIpcError::Authentication)
}

async fn receive_json<S>(socket: &mut WebSocketStream<S>) -> Result<Value, LocalIpcError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match receive_frame(socket).await? {
        Message::Text(text) => Ok(serde_json::from_str(&text)?),
        _ => Err(LocalIpcError::Protocol),
    }
}

async fn receive_frame<S>(socket: &mut WebSocketStream<S>) -> Result<Message, LocalIpcError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        match socket.next().await {
            Some(Ok(Message::Ping(payload))) => socket.send(Message::Pong(payload)).await?,
            Some(Ok(Message::Close(_))) | None => return Err(LocalIpcError::Closed),
            Some(Ok(frame)) => return Ok(frame),
            Some(Err(error)) => return Err(error.into()),
        }
    }
}

async fn send_json<S>(socket: &mut WebSocketStream<S>, value: &Value) -> Result<(), LocalIpcError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    socket.send(Message::Text(value.to_string().into())).await?;
    Ok(())
}

async fn authenticate_server(
    socket: &mut WebSocketStream<TcpStream>,
    token: &[u8; 32],
) -> Result<(), LocalIpcError> {
    let mut nonce = [0u8; 32];
    OsRng.fill_bytes(&mut nonce);
    send_json(
        socket,
        &json!({"type":"hello","nonce":hex::encode(nonce),"proof":hex::encode(proof(token,b"server",&nonce))}),
    )
    .await?;
    let reply = tokio::time::timeout(AUTH_TIMEOUT, receive_json(socket))
        .await
        .map_err(|_| LocalIpcError::Timeout)??;
    if reply["type"] != "authenticate" {
        return Err(LocalIpcError::Authentication);
    }
    verify_proof(
        token,
        b"client",
        &nonce,
        reply["proof"]
            .as_str()
            .ok_or(LocalIpcError::Authentication)?,
    )?;
    send_json(socket, &json!({"type":"authenticated"})).await
}

async fn authenticate_client(
    socket: &mut LocalSocket,
    token: &[u8; 32],
) -> Result<(), LocalIpcError> {
    let hello = tokio::time::timeout(AUTH_TIMEOUT, receive_json(socket))
        .await
        .map_err(|_| LocalIpcError::Timeout)??;
    if hello["type"] != "hello" {
        return Err(LocalIpcError::Authentication);
    }
    let nonce_text = hello["nonce"]
        .as_str()
        .ok_or(LocalIpcError::Authentication)?;
    let mut nonce = [0u8; 32];
    hex::decode_to_slice(nonce_text, &mut nonce).map_err(|_| LocalIpcError::Authentication)?;
    verify_proof(
        token,
        b"server",
        &nonce,
        hello["proof"]
            .as_str()
            .ok_or(LocalIpcError::Authentication)?,
    )?;
    send_json(
        socket,
        &json!({"type":"authenticate","proof":hex::encode(proof(token,b"client",&nonce))}),
    )
    .await?;
    let accepted = tokio::time::timeout(AUTH_TIMEOUT, receive_json(socket))
        .await
        .map_err(|_| LocalIpcError::Timeout)??;
    if accepted["type"] != "authenticated" {
        return Err(LocalIpcError::Authentication);
    }
    Ok(())
}

pub async fn connect_local() -> Result<LocalSocket, LocalIpcError> {
    let token = read_token(&user_token_path()?)?;
    connect_with_token(local_port()?, &token).await
}

async fn connect_with_token(port: u16, token: &[u8; 32]) -> Result<LocalSocket, LocalIpcError> {
    let url = format!("ws://127.0.0.1:{port}/local");
    let (mut socket, _) = tokio::time::timeout(AUTH_TIMEOUT, connect_async(url))
        .await
        .map_err(|_| LocalIpcError::Timeout)??;
    authenticate_client(&mut socket, token).await?;
    Ok(socket)
}

pub async fn next_status(socket: &mut LocalSocket) -> Result<crate::DeviceStatus, LocalIpcError> {
    loop {
        let frame = receive_json(socket).await?;
        if frame["type"] == "status" {
            return serde_json::from_value(frame["status"].clone()).map_err(Into::into);
        }
        if frame["type"] == "error" {
            return Err(LocalIpcError::Remote(
                frame["message"]
                    .as_str()
                    .unwrap_or("local service error")
                    .to_owned(),
            ));
        }
    }
}

pub async fn approve_local_claim(claim_id: ClaimId) -> Result<(), LocalIpcError> {
    let mut socket = connect_local().await?;
    send_json(
        &mut socket,
        &json!({"type":"approve_claim","claim_id":claim_id.to_string()}),
    )
    .await?;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let frame = receive_json(&mut socket).await?;
            match frame["type"].as_str() {
                Some("approved") => return Ok(()),
                Some("error") => {
                    return Err(LocalIpcError::Remote(
                        frame["message"]
                            .as_str()
                            .unwrap_or("approval failed")
                            .to_owned(),
                    ));
                }
                _ => {}
            }
        }
    })
    .await
    .map_err(|_| LocalIpcError::Timeout)?
}

pub async fn run_local_service(root: PathBuf) -> Result<(), LocalIpcError> {
    let token = ensure_machine_token(&root)?;
    let port = local_port()?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    tracing::info!(port, "local device WebSocket ready");
    serve(listener, root, token).await
}

pub fn spawn_local_service(root: PathBuf) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Err(error) = run_local_service(root.clone()).await {
                tracing::warn!(%error, "local WebSocket service unavailable; retrying in 3s");
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    })
}

async fn serve(listener: TcpListener, root: PathBuf, token: [u8; 32]) -> Result<(), LocalIpcError> {
    let connection_limit = Arc::new(Semaphore::new(32));
    loop {
        let (stream, address) = listener.accept().await?;
        if !address.ip().is_loopback() {
            continue;
        }
        let Ok(permit) = connection_limit.clone().try_acquire_owned() else {
            continue;
        };
        let root = root.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) = handle_connection(stream, &root, &token).await {
                tracing::debug!(%error, "local WebSocket connection ended");
            }
        });
    }
}

async fn handle_connection(
    stream: TcpStream,
    root: &Path,
    token: &[u8; 32],
) -> Result<(), LocalIpcError> {
    let mut socket = tokio::time::timeout(AUTH_TIMEOUT, accept_async(stream))
        .await
        .map_err(|_| LocalIpcError::Timeout)??;
    authenticate_server(&mut socket, token).await?;
    let mut updates = tokio::time::interval(Duration::from_secs(3));
    let mut helper_id = None;
    let (requests, mut receiver) = mpsc::channel::<HelperRequest>(1);
    let mut pending: Option<PendingRequest> = None;
    let result = loop {
        tokio::select! {
            _ = updates.tick() => {
                match read_device_status_from_service(root).await {
                    Ok(status) => send_json(&mut socket, &json!({"type":"status","status":status})).await?,
                    Err(error) => send_json(&mut socket, &json!({"type":"error","message":error.to_string()})).await?,
                }
            }
            frame = receive_frame(&mut socket) => {
                let frame = match frame { Ok(frame) => frame, Err(error) => break Err(error) };
                if let Message::Binary(bytes) = frame {
                    let Some(PendingRequest::Screenshot(reply)) = pending.take() else {
                        break Err(LocalIpcError::Protocol);
                    };
                    let result = validate_png(&bytes).map(|_| bytes.to_vec());
                    let _ = reply.send(result);
                    continue;
                }
                let Message::Text(text) = frame else { break Err(LocalIpcError::Protocol) };
                let frame: Value = serde_json::from_str(&text)?;
                match frame["type"].as_str() {
                    Some("approve_claim") => {
                        let claim_id = frame["claim_id"]
                            .as_str()
                            .ok_or(LocalIpcError::Protocol)?
                            .parse::<ClaimId>()
                            .map_err(|_| LocalIpcError::Protocol)?;
                        match approve_claim(claim_id).await {
                            Ok(()) => send_json(&mut socket, &json!({"type":"approved"})).await?,
                            Err(error) => send_json(&mut socket, &json!({"type":"error","message":error.to_string()})).await?,
                        }
                    }
                    Some("register_window_helper") if helper_id.is_none() => {
                        let id = NEXT_PROVIDER_ID.fetch_add(1, Ordering::Relaxed);
                        window_providers().lock().map_err(|_| LocalIpcError::Protocol)?.push(WindowProvider { id, sender: requests.clone() });
                        helper_id = Some(id);
                        tracing::info!(helper_id = id, "interactive window helper registered");
                        send_json(&mut socket, &json!({"type":"window_helper_registered"})).await?;
                    }
                    Some("window_list") if matches!(pending, Some(PendingRequest::Windows(_))) => {
                        let entries: Result<Vec<WindowEntry>, _> = serde_json::from_value(frame["entries"].clone());
                        let result = entries.map_err(LocalIpcError::from).and_then(|entries| {
                            if entries.len() > MAX_WINDOW_ENTRIES || entries.iter().any(|entry| entry.title.is_empty() || entry.title.len() > MAX_WINDOW_TITLE_BYTES) {
                                Err(LocalIpcError::Protocol)
                            } else { Ok(entries) }
                        });
                        if let Some(PendingRequest::Windows(reply)) = pending.take() { let _ = reply.send(result); }
                    }
                    Some("screenshot_error") if matches!(pending, Some(PendingRequest::Screenshot(_))) => {
                        if let Some(PendingRequest::Screenshot(reply)) = pending.take() {
                            let message = frame["message"].as_str().unwrap_or("screenshot unavailable");
                            let _ = reply.send(Err(LocalIpcError::Remote(message.to_owned())));
                        }
                    }
                    Some("desktop_input_result") if matches!(pending, Some(PendingRequest::DesktopInput(_))) => {
                        if let Some(PendingRequest::DesktopInput(reply)) = pending.take() {
                            let result = if frame["ok"] == true {
                                Ok(())
                            } else {
                                Err(LocalIpcError::Remote(frame["message"].as_str().unwrap_or("desktop input failed").to_owned()))
                            };
                            let _ = reply.send(result);
                        }
                    }
                    _ => break Err(LocalIpcError::Protocol),
                }
            }
            request = receiver.recv(), if helper_id.is_some() && pending.is_none() => {
                if let Some(request) = request {
                    match request {
                        HelperRequest::Windows(reply) => {
                            send_json(&mut socket, &json!({"type":"list_windows"})).await?;
                            pending = Some(PendingRequest::Windows(reply));
                        }
                        HelperRequest::Screenshot(reply) => {
                            send_json(&mut socket, &json!({"type":"capture_screenshot"})).await?;
                            pending = Some(PendingRequest::Screenshot(reply));
                        }
                        HelperRequest::DesktopInput(event, reply) => {
                            send_json(&mut socket, &json!({"type":"desktop_input","event":event})).await?;
                            pending = Some(PendingRequest::DesktopInput(reply));
                        }
                    }
                }
            }
        }
    };
    if let Some(id) = helper_id {
        window_providers()
            .lock()
            .map_err(|_| LocalIpcError::Protocol)?
            .retain(|provider| provider.id != id);
    }
    result
}

#[derive(Debug, Error)]
pub enum LocalIpcError {
    #[error(transparent)]
    DataPath(#[from] pab_agent_core::DataPathError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("PAB_LOCAL_IPC_PORT is invalid")]
    InvalidPort,
    #[error("local access token is invalid")]
    InvalidToken,
    #[error("local WebSocket authentication failed")]
    Authentication,
    #[error("local WebSocket timed out")]
    Timeout,
    #[error("local WebSocket closed")]
    Closed,
    #[error("local WebSocket message is invalid")]
    Protocol,
    #[error("local device service: {0}")]
    Remote(String),
    #[error("interactive window helper is unavailable")]
    WindowHelperUnavailable,
    #[error("interactive screenshot is invalid or exceeds the size limit")]
    InvalidScreenshot,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_HELPER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn authenticated_client_receives_status_and_rejects_wrong_token() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        crate::device_access::save(
            &root.join("executor.sqlite3"),
            &crate::device_access::DeviceAccess {
                deployment_id: "deployment".to_owned(),
                tenant_id: "tenant".to_owned(),
                device_id: "device-id".to_owned(),
                device_code: "123456789".to_owned(),
                temporary_password: "password".to_owned(),
                password_version: 1,
                password_hash: "hash".to_owned(),
            },
        )
        .await
        .unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        fs::write(
            root.join("executor-heartbeat.json"),
            json!({"observed_at_unix_ms":now,"control_phase":"Connected"}).to_string(),
        )
        .unwrap();
        let token = ensure_machine_token(root).unwrap();
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(serve(listener, root.to_path_buf(), token));

        let mut socket = connect_with_token(port, &token).await.unwrap();
        let status = next_status(&mut socket).await.unwrap();
        assert_eq!(status.device_code, "123456789");
        assert_eq!(status.temporary_password, "password");
        assert!(status.executor_running);

        let wrong_token = [0u8; 32];
        assert!(connect_with_token(port, &wrong_token).await.is_err());
        server.abort();
    }

    #[tokio::test]
    async fn interactive_helper_serves_a_window_request() {
        let _guard = TEST_HELPER_LOCK.lock().await;
        let directory = tempfile::tempdir().unwrap();
        let token = ensure_machine_token(directory.path()).unwrap();
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(serve(listener, directory.path().to_path_buf(), token));
        let mut socket = connect_with_token(port, &token).await.unwrap();
        register_window_helper(&mut socket).await.unwrap();
        let (done, wait_for_result) = oneshot::channel::<()>();
        let helper = tokio::spawn(async move {
            loop {
                match next_local_event(&mut socket).await {
                    Ok(LocalEvent::ListWindows) => {
                        reply_window_list(
                            &mut socket,
                            &[WindowEntry {
                                title: "Editor".to_owned(),
                                process_id: 42,
                            }],
                        )
                        .await
                        .unwrap();
                        let _ = wait_for_result.await;
                        break;
                    }
                    Ok(LocalEvent::Status(_)) | Ok(LocalEvent::StatusUnavailable(_)) => {}
                    Ok(LocalEvent::CaptureScreenshot) => panic!("unexpected screenshot request"),
                    Ok(LocalEvent::DesktopInput(_)) => panic!("unexpected desktop input"),
                    Err(error) => panic!("helper connection failed: {error}"),
                }
            }
        });
        let result = request_window_list().await.unwrap();
        assert_eq!(result[0].title, "Editor");
        assert_eq!(result[0].process_id, 42);
        let _ = done.send(());
        helper.await.unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn disconnecting_newer_helper_restores_the_older_helper() {
        let _guard = TEST_HELPER_LOCK.lock().await;
        let directory = tempfile::tempdir().unwrap();
        let token = ensure_machine_token(directory.path()).unwrap();
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(serve(listener, directory.path().to_path_buf(), token));

        let mut first = connect_with_token(port, &token).await.unwrap();
        register_window_helper(&mut first).await.unwrap();
        let first_id = window_providers().lock().unwrap().last().unwrap().id;
        let responder = tokio::spawn(async move {
            loop {
                match next_local_event(&mut first).await.unwrap() {
                    LocalEvent::ListWindows => {
                        reply_window_list(
                            &mut first,
                            &[WindowEntry {
                                title: "Older helper".to_owned(),
                                process_id: 7,
                            }],
                        )
                        .await
                        .unwrap();
                        break;
                    }
                    LocalEvent::Status(_) | LocalEvent::StatusUnavailable(_) => {}
                    LocalEvent::CaptureScreenshot => panic!("unexpected screenshot request"),
                    LocalEvent::DesktopInput(_) => panic!("unexpected desktop input"),
                }
            }
        });

        let mut second = connect_with_token(port, &token).await.unwrap();
        register_window_helper(&mut second).await.unwrap();
        let second_id = window_providers().lock().unwrap().last().unwrap().id;
        assert_ne!(first_id, second_id);
        drop(second);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if window_providers().lock().unwrap().last().unwrap().id == first_id {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        let result = request_window_list().await.unwrap();
        assert_eq!(result[0].title, "Older helper");
        responder.await.unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn desktop_input_reaches_registered_helper() {
        let _guard = TEST_HELPER_LOCK.lock().await;
        let directory = tempfile::tempdir().unwrap();
        let token = ensure_machine_token(directory.path()).unwrap();
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(serve(listener, directory.path().to_path_buf(), token));
        let mut socket = connect_with_token(port, &token).await.unwrap();
        register_window_helper(&mut socket).await.unwrap();
        let (done, received) = oneshot::channel::<()>();
        let helper = tokio::spawn(async move {
            loop {
                match next_local_event(&mut socket).await.unwrap() {
                    LocalEvent::DesktopInput(event) => {
                        assert_eq!(
                            event,
                            DesktopInputEvent::Key {
                                virtual_key: 0x41,
                                down: true,
                            }
                        );
                        reply_desktop_input(&mut socket, Ok(())).await.unwrap();
                        // Keep the WebSocket alive until the service consumes
                        // the reply; dropping a socket with unread status frames
                        // can reset TCP on Windows under parallel test load.
                        let _ = received.await;
                        break;
                    }
                    LocalEvent::Status(_) | LocalEvent::StatusUnavailable(_) => {}
                    _ => panic!("unexpected helper request"),
                }
            }
        });
        request_desktop_input(DesktopInputEvent::Key {
            virtual_key: 0x41,
            down: true,
        })
        .await
        .unwrap();
        let _ = done.send(());
        helper.await.unwrap();
        server.abort();
    }
    #[test]
    fn screenshot_frame_rejects_empty_and_oversized_images() {
        assert!(matches!(
            validate_png(&[]),
            Err(LocalIpcError::InvalidScreenshot)
        ));
        let mut oversized = vec![0u8; MAX_SCREENSHOT_BYTES + 1];
        oversized[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        oversized[12..16].copy_from_slice(b"IHDR");
        oversized[19] = 1;
        oversized[23] = 1;
        assert!(matches!(
            validate_png(&oversized),
            Err(LocalIpcError::InvalidScreenshot)
        ));
    }
}
