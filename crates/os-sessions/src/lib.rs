//! Read-only native login-session inventory. Unsafe WTS ownership is isolated here.
use pab_protocol::OsSessionInfo;

pub struct SessionBatch {
    pub backend: &'static str,
    pub entries: Vec<OsSessionInfo>,
    pub truncated: bool,
}
#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub async fn collect() -> Result<SessionBatch, String> {
    #[cfg(windows)]
    {
        tokio::task::spawn_blocking(windows::collect)
            .await
            .map_err(|e| format!("WTS worker: {e}"))?
    }
    #[cfg(target_os = "linux")]
    {
        tokio::time::timeout(std::time::Duration::from_secs(5), linux::collect())
            .await
            .map_err(|_| "logind collection timed out".to_string())?
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Err("unsupported OS session backend".into())
    }
}
#[cfg(any(windows, target_os = "linux"))]
fn bounded(s: &str) -> String {
    s.chars().take(256).collect()
}
