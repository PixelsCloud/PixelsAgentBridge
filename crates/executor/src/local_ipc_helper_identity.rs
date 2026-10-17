//! Reuse the existing kernel-verified worker channel as a registration challenge.
//! Registration usernames/session fields are never trusted to select an account.
use super::*;
use pab_os_control::execution::{UserIdentity, channel, process_user_identity};
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
    // Linux media is additive. Preserve the existing application-registration
    // policy until Linux application routing has its own acceptance.
    if !cfg!(any(windows, target_os = "macos")) {
        return Ok(None);
    }
    let Some(verified) = verify_process(socket, frame).await? else {
        return Ok(None);
    };
    let account = verified.native.account_id.as_str();
    // Existing application routing policy is intentionally unchanged.
    if matches!(account, "uid:0" | "S-1-5-18" | "S-1-5-19" | "S-1-5-20")
        || verified.native.session_id.is_none_or(|s| s == 0)
    {
        return Ok(None);
    }
    let identity = verified
        .native
        .observation(
            ExecutionMode::DesktopUser,
            ExecutionEnvironmentSource::InteractiveSession,
        )
        .map_err(|_| LocalIpcError::Protocol)?;
    Ok(Some(VerifiedHelper {
        identity,
        process_id: verified.pid,
        process_identity: verified.marker,
    }))
}

pub(super) struct VerifiedMediaHelper {
    native: UserIdentity,
    pid: u32,
    marker: String,
    #[cfg(target_os = "linux")]
    graphical: Option<pab_os_control::execution::LinuxGraphicalSession>,
}
impl VerifiedMediaHelper {
    pub fn same_process(&self, other: &Self) -> bool {
        let same = self.pid == other.pid && self.marker == other.marker && self.native == other.native;
        #[cfg(target_os = "linux")]
        { same && self.graphical == other.graphical }
        #[cfg(not(target_os = "linux"))]
        { same }
    }
    pub fn current(&self) -> bool {
        let current = pab_os_control::process_identity(self.pid).ok().as_ref() == Some(&self.marker)
            && process_user_identity(self.pid).ok().as_ref() == Some(&self.native);
        #[cfg(target_os = "linux")]
        { current && self.graphical.as_ref().is_some_and(|session| session.current(self.pid)) }
        #[cfg(not(target_os = "linux"))]
        { current }
    }
    pub async fn wait_retired(&self) {
        #[cfg(target_os = "linux")]
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if !self.current() { return; }
        }
        #[cfg(not(target_os = "linux"))]
        std::future::pending::<()>().await;
    }
}
#[cfg(any(not(target_os = "linux"), test))]
fn media_account_allowed(account: &str, session: Option<u32>) -> bool {
    session.is_some_and(|id| id != 0)
        && !matches!(account, "S-1-5-19" | "S-1-5-20")
        && (account != "uid:0" || cfg!(target_os = "macos"))
        && (account != "S-1-5-18" || cfg!(windows))
}
pub(super) async fn verify_media(
    socket: &mut WebSocketStream<TcpStream>,
    frame: &Value,
) -> Result<Option<VerifiedMediaHelper>, LocalIpcError> {
    let Some(verified) = verify_process(socket, frame).await? else {
        return Ok(None);
    };
    #[cfg(target_os = "linux")]
    {
        let mut verified = verified;
        let uid = verified.native.account_id.strip_prefix("uid:").and_then(|s| s.parse().ok());
        let Some(uid) = uid else { return Ok(None); };
        let pid = verified.pid;
        verified.graphical = tokio::task::spawn_blocking(move || {
            pab_os_control::execution::LinuxGraphicalSession::for_media_process(pid, uid).ok()
        }).await.map_err(|_| LocalIpcError::Protocol)?;
        return Ok(verified.current().then_some(verified));
    }
    // The existing supervisor launches the Winlogon helper as SYSTEM in the
    // actual interactive session. Allow that identity only for media; never
    // expose it as a DesktopUser/application execution context. Session 0 and
    // Local/NetworkService remain rejected. macOS LoginWindow likewise uses
    // the existing root helper in its graphical audit session. Application
    // execution registration above still rejects root. Identity is kernel read.
    #[cfg(not(target_os = "linux"))]
    Ok(
        media_account_allowed(&verified.native.account_id, verified.native.session_id)
            .then_some(verified),
    )
}
async fn verify_process(
    socket: &mut WebSocketStream<TcpStream>,
    frame: &Value,
) -> Result<Option<VerifiedMediaHelper>, LocalIpcError> {
    let Some(pid) = frame["process_id"]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
    else {
        return Ok(None);
    };
    if !cfg!(any(windows, target_os = "macos", target_os = "linux")) {
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
    Ok(Some(VerifiedMediaHelper {
        native,
        pid,
        marker,
        #[cfg(target_os = "linux")]
        graphical: None,
    }))
}

#[cfg(test)]
mod media_tests {
    use super::*;
    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn media_pairing_binds_process_lifetime_and_session() {
        let pid = std::process::id();
        let native = process_user_identity(pid).unwrap();
        let first = VerifiedMediaHelper {
            native: native.clone(),
            pid,
            marker: "first-lifetime".into(),
        };
        let mut other = VerifiedMediaHelper {
            native,
            pid,
            marker: "first-lifetime".into(),
        };
        assert!(first.same_process(&other));
        other.marker = "reused-pid".into();
        assert!(!first.same_process(&other));
        other.marker = first.marker.clone();
        other.pid = pid.saturating_add(1);
        assert!(!first.same_process(&other));
        other.pid = pid;
        other.native.session_id = Some(u32::MAX);
        assert!(!first.same_process(&other));
    }
    #[test]
    fn media_requires_interactive_session_and_does_not_admit_service_accounts() {
        assert!(!media_account_allowed("S-1-5-18", Some(0)));
        assert!(!media_account_allowed("user", None));
        assert!(!media_account_allowed("S-1-5-19", Some(2)));
        assert!(!media_account_allowed("S-1-5-20", Some(2)));
        assert_eq!(media_account_allowed("uid:0", Some(2)), cfg!(target_os = "macos"));
        assert!(!media_account_allowed("uid:0", Some(0)));
        assert!(media_account_allowed("user", Some(2)));
        assert_eq!(media_account_allowed("S-1-5-18", Some(2)), cfg!(windows));
    }
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
