//! Uses the protected loopback Executor service, never the public Desktop status port.
use super::{AccountClient, AccountError, AccountStore};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use pab_protocol::{
    DeviceAccountAction, DeviceAccountChallengeRequest, DeviceAccountProof, DeviceAccountState,
    DeviceId,
};
use serde_json::{Value, json};
use sha2::Sha256;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use zeroize::Zeroizing;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn read(socket: &mut Socket) -> Result<Value, AccountError> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(value))) => {
                return serde_json::from_str(&value).map_err(|_| AccountError::InvalidResponse);
            }
            Some(Ok(Message::Ping(value))) => socket
                .send(Message::Pong(value))
                .await
                .map_err(|_| AccountError::Network)?,
            Some(Ok(Message::Pong(_))) => {}
            _ => return Err(AccountError::Network),
        }
    }
}

async fn send(socket: &mut Socket, value: Value) -> Result<(), AccountError> {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .map_err(|_| AccountError::Network)
}

pub async fn associate(
    action: DeviceAccountAction,
    expected_revision: Option<i64>,
) -> Result<Option<DeviceAccountState>, AccountError> {
    associate_at(action, expected_revision, None).await
}

pub async fn associate_at(
    action: DeviceAccountAction,
    expected_revision: Option<i64>,
    account_revision: Option<u64>,
) -> Result<Option<DeviceAccountState>, AccountError> {
    tokio::time::timeout(
        Duration::from_secs(40),
        associate_inner(action, expected_revision, account_revision),
    )
    .await
    .map_err(|_| AccountError::Network)?
}

async fn associate_inner(
    action: DeviceAccountAction,
    expected_revision: Option<i64>,
    account_revision: Option<u64>,
) -> Result<Option<DeviceAccountState>, AccountError> {
    let store = AccountStore::from_env()?;
    let account = store.read()?;
    if account_revision.is_some_and(|revision| revision != account.revision) {
        return Err(AccountError::Storage);
    }
    let Some(user) = account.user.as_ref() else {
        return Ok(None);
    };
    let token = store
        .token(
            account
                .active_slot
                .as_deref()
                .ok_or(AccountError::Storage)?,
        )?
        .ok_or(AccountError::Storage)?;
    let client = AccountClient::from_env()?;
    let path = crate::DataPaths::for_scope(crate::DataScope::User)
        .map_err(|_| AccountError::Storage)?
        .root()
        .join("local-access.key");
    let encoded = Zeroizing::new(std::fs::read_to_string(path).map_err(|_| AccountError::Storage)?);
    let mut key = Zeroizing::new([0u8; 32]);
    hex::decode_to_slice(encoded.trim(), key.as_mut()).map_err(|_| AccountError::Storage)?;
    let port = std::env::var("PAB_LOCAL_IPC_PORT")
        .unwrap_or_else(|_| "7843".into())
        .parse::<u16>()
        .map_err(|_| AccountError::InvalidResponse)?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(128 * 1024))
        .max_frame_size(Some(128 * 1024));
    let (mut socket, _) =
        connect_async_with_config(format!("ws://127.0.0.1:{port}/local"), Some(config), false)
            .await
            .map_err(|_| AccountError::Network)?;
    let hello = read(&mut socket).await?;
    if hello["type"] != "hello" {
        return Err(AccountError::InvalidResponse);
    }
    let nonce = hex::decode(
        hello["nonce"]
            .as_str()
            .ok_or(AccountError::InvalidResponse)?,
    )
    .map_err(|_| AccountError::InvalidResponse)?;
    if nonce.len() != 32 {
        return Err(AccountError::InvalidResponse);
    }
    let proof = hex::decode(
        hello["proof"]
            .as_str()
            .ok_or(AccountError::InvalidResponse)?,
    )
    .map_err(|_| AccountError::InvalidResponse)?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_ref()).map_err(|_| AccountError::InvalidResponse)?;
    mac.update(b"server");
    mac.update(&nonce);
    mac.verify_slice(&proof)
        .map_err(|_| AccountError::InvalidResponse)?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_ref()).map_err(|_| AccountError::InvalidResponse)?;
    mac.update(b"client");
    mac.update(&nonce);
    send(
        &mut socket,
        json!({"type":"authenticate","proof":hex::encode(mac.finalize().into_bytes())}),
    )
    .await?;
    if read(&mut socket).await?["type"] != "authenticated" {
        return Err(AccountError::InvalidResponse);
    }
    let device: DeviceId = loop {
        let frame = read(&mut socket).await?;
        if frame["type"] == "status" {
            break frame["status"]["device_id"]
                .as_str()
                .ok_or(AccountError::InvalidResponse)?
                .parse()
                .map_err(|_| AccountError::InvalidResponse)?;
        }
        if frame["type"] == "error" {
            return Err(AccountError::Network);
        }
    };
    let state = client
        .device_association_challenge(
            &token,
            device,
            &DeviceAccountChallengeRequest {
                action,
                expected_revision,
            },
        )
        .await?;
    let Some(challenge) = state.challenge.as_ref() else {
        return Ok(Some(state));
    };
    if challenge.server_origin != client.origin()
        || challenge.user_id != user.id
        || challenge.device_id != device
        || challenge.action != action
    {
        return Err(AccountError::InvalidResponse);
    }
    if store.read()?.revision != account.revision {
        return Err(AccountError::Storage);
    }
    send(
        &mut socket,
        json!({"type":"sign_device_account","challenge":challenge}),
    )
    .await?;
    let proof: DeviceAccountProof = loop {
        let frame = read(&mut socket).await?;
        match frame["type"].as_str() {
            Some("device_account_proof") => {
                break serde_json::from_value(frame["proof"].clone())
                    .map_err(|_| AccountError::InvalidResponse)?;
            }
            Some("error") => return Err(AccountError::InvalidResponse),
            Some("status") => {}
            _ => return Err(AccountError::InvalidResponse),
        }
    };
    if proof.challenge_id != challenge.id || store.read()?.revision != account.revision {
        return Err(AccountError::Storage);
    }
    let result = client.associate_device(&token, device, &proof).await?;
    let _ = socket.close(None).await;
    Ok(Some(result))
}
