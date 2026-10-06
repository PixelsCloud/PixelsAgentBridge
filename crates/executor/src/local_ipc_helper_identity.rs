//! Reuse the existing kernel-verified worker channel as a registration challenge.
//! Registration usernames/session fields are never trusted to select an account.
use super::*;
use pab_os_control::execution::{channel, process_user_identity};
use pab_protocol::{ExecutionEnvironmentSource, ExecutionIdentity, ExecutionMode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub(super) struct VerifiedHelper {
    pub identity: ExecutionIdentity,
    pub process_id: u32,
    pub process_identity: String,
}
impl VerifiedHelper {
    pub fn current(&self) -> bool {
        pab_os_control::process_identity(self.process_id)
            .ok()
            .as_ref()
            == Some(&self.process_identity)
            && process_user_identity(self.process_id)
                .ok()
                .and_then(|i| {
                    i.observation(
                        ExecutionMode::DesktopUser,
                        ExecutionEnvironmentSource::InteractiveSession,
                    )
                    .ok()
                })
                .as_ref()
                == Some(&self.identity)
    }
}
pub(super) async fn answer(frame: &Value) -> Result<(), LocalIpcError> {
    let address = frame["address"].as_str().ok_or(LocalIpcError::Protocol)?;
    let pid = frame["server_pid"]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or(LocalIpcError::Protocol)?;
    let nonce = frame["nonce"]
        .as_str()
        .filter(|n| n.len() == 36)
        .ok_or(LocalIpcError::Protocol)?;
    let mut peer = channel::connect(address, pid, AUTH_TIMEOUT).await?;
    peer.write_all(nonce.as_bytes()).await?;
    if peer.read_u8().await? != 1 {
        return Err(LocalIpcError::Protocol);
    }
    Ok(())
}
pub(super) async fn verify(
    socket: &mut WebSocketStream<TcpStream>,
    frame: &Value,
) -> Result<Option<VerifiedHelper>, LocalIpcError> {
    let Some(pid) = frame["process_id"]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
    else {
        return Ok(None);
    };
    // Linux headless still supports old diagnostic helpers, not user app routing.
    if !cfg!(any(windows, target_os = "macos")) {
        return Ok(None);
    }
    let (native, marker) = tokio::task::spawn_blocking(move || {
        let marker = pab_os_control::process_identity(pid).map_err(std::io::Error::other)?;
        let native = process_user_identity(pid)?;
        Ok::<_, std::io::Error>((native, marker))
    })
    .await
    .map_err(|_| LocalIpcError::Protocol)??;
    let mut listener = channel::WorkerListener::bind(&native)?;
    let nonce = pab_protocol::RequestId::new().to_string();
    send_json(socket,&json!({"type":"verify_helper_identity","address":listener.address(),"server_pid":std::process::id(),"nonce":nonce})).await?;
    let mut peer = listener.accept(pid, AUTH_TIMEOUT).await?;
    let mut received = [0; 36];
    tokio::time::timeout(AUTH_TIMEOUT, peer.read_exact(&mut received))
        .await
        .map_err(|_| LocalIpcError::Timeout)??;
    if received != nonce.as_bytes()
        || pab_os_control::process_identity(pid).ok().as_ref() != Some(&marker)
        || process_user_identity(pid).ok().as_ref() != Some(&native)
    {
        return Err(LocalIpcError::Protocol);
    }
    peer.write_u8(1).await?;
    let account = native.account_id.as_str();
    if matches!(account, "uid:0" | "S-1-5-18" | "S-1-5-19" | "S-1-5-20")
        || native.session_id.is_none_or(|s| s == 0)
    {
        return Ok(None);
    }
    let identity = native
        .observation(
            ExecutionMode::DesktopUser,
            ExecutionEnvironmentSource::InteractiveSession,
        )
        .map_err(|_| LocalIpcError::Protocol)?;
    Ok(Some(VerifiedHelper {
        identity,
        process_id: pid,
        process_identity: marker,
    }))
}

pub(crate) fn desktop_identities() -> Vec<ExecutionIdentity> {
    let providers = window_providers()
        .lock()
        .map(|p| {
            p.iter()
                .filter(|p| {
                    p.application_schema_version.is_some_and(|v| v >= 1) && !p.sender.is_closed()
                })
                .filter_map(|p| p.identity.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut result = Vec::new();
    for provider in providers {
        if provider.current() && !result.contains(&provider.identity) {
            result.push(provider.identity);
        }
    }
    result
}
