use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use pab_agent_core::{DataPaths, DataScope, ensure_data_parent, restrict_private_file};
use pab_protocol::ClaimId;
use rand_core::{OsRng, RngCore};
use serde_json::{Value, json};
use sha2::Sha256;
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
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
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => return Ok(serde_json::from_str(&text)?),
            Some(Ok(Message::Ping(payload))) => socket.send(Message::Pong(payload)).await?,
            Some(Ok(Message::Close(_))) | None => return Err(LocalIpcError::Closed),
            Some(Ok(_)) => return Err(LocalIpcError::Protocol),
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
    loop {
        tokio::select! {
            _ = updates.tick() => {
                match read_device_status_from_service(root) {
                    Ok(status) => send_json(&mut socket, &json!({"type":"status","status":status})).await?,
                    Err(error) => send_json(&mut socket, &json!({"type":"error","message":error.to_string()})).await?,
                }
            }
            frame = receive_json(&mut socket) => {
                let frame = frame?;
                if frame["type"] != "approve_claim" {
                    return Err(LocalIpcError::Protocol);
                }
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
        }
    }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn authenticated_client_receives_status_and_rejects_wrong_token() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(
            root.join("device-info.json"),
            json!({"device_code":"123456789","device_id":"device-id"}).to_string(),
        )
        .unwrap();
        fs::write(root.join("current-password.txt"), "password\n").unwrap();
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
}
