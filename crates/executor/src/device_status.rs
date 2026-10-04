use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
use std::{
    env,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

#[cfg(windows)]
use windows_service::{
    service::{ServiceAccess, ServiceState},
    service_manager::{ServiceManager, ServiceManagerAccess},
};

use pab_agent_core::{DataPaths, DataScope};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeviceStatus {
    pub device_code: String,
    pub device_id: String,
    pub temporary_password: String,
    pub executor_running: bool,
    pub control_phase: String,
}

pub async fn read_local_device_status() -> Result<DeviceStatus, DeviceStatusError> {
    let paths = DataPaths::for_scope(DataScope::Machine)?;
    read_device_status(paths.root()).await
}

pub(crate) async fn read_device_status(root: &Path) -> Result<DeviceStatus, DeviceStatusError> {
    read_device_status_with_service(root, false).await
}

pub(crate) async fn read_device_status_from_service(
    root: &Path,
) -> Result<DeviceStatus, DeviceStatusError> {
    read_device_status_with_service(root, true).await
}

async fn read_device_status_with_service(
    root: &Path,
    service_alive: bool,
) -> Result<DeviceStatus, DeviceStatusError> {
    let access = crate::device_access::load(&root.join("executor.sqlite3")).await?;
    let heartbeat: Option<Value> = fs::read(root.join("executor-heartbeat.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let observed = heartbeat
        .as_ref()
        .and_then(|value| value.get("observed_at_unix_ms"))
        .and_then(Value::as_u64);
    let now = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())
        .map_err(|_| DeviceStatusError::ClockOverflow)?;
    let heartbeat_running = observed
        .map(|observed| now.saturating_sub(observed) <= 10_000)
        .unwrap_or(false);
    let control_phase = heartbeat
        .as_ref()
        .filter(|_| heartbeat_running)
        .and_then(|value| value.get("control_phase"))
        .and_then(Value::as_str)
        .unwrap_or("Unknown")
        .to_owned();
    Ok(DeviceStatus {
        device_code: access.device_code,
        device_id: access.device_id,
        temporary_password: access.temporary_password,
        executor_running: service_alive || heartbeat_running || service_running(),
        control_phase,
    })
}

#[cfg(windows)]
fn service_running() -> bool {
    static CACHE: OnceLock<Mutex<Option<(Instant, bool)>>> = OnceLock::new();
    let Ok(mut cached) = CACHE.get_or_init(|| Mutex::new(None)).lock() else {
        return false;
    };
    if let Some((checked_at, running)) = *cached
        && checked_at.elapsed() < Duration::from_secs(30)
    {
        return running;
    }

    let service_name = env::var("PAB_EXECUTOR_SERVICE_NAME")
        .unwrap_or_else(|_| "PixelsAgentBridgeExecutor".to_owned());
    let running = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .and_then(|manager| manager.open_service(service_name, ServiceAccess::QUERY_STATUS))
        .and_then(|service| service.query_status())
        .is_ok_and(|status| status.current_state == ServiceState::Running);
    *cached = Some((Instant::now(), running));
    running
}

#[cfg(not(windows))]
fn service_running() -> bool {
    false
}

#[derive(Debug, Error)]
pub enum DeviceStatusError {
    #[error(transparent)]
    DataPath(#[from] pab_agent_core::DataPathError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Access(#[from] crate::device_access::DeviceAccessError),
    #[error(transparent)]
    Clock(#[from] std::time::SystemTimeError),
    #[error("device information has no {0}")]
    MissingField(&'static str),
    #[error("system clock value is too large")]
    ClockOverflow,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn reads_device_access_and_live_status() {
        let root = tempfile::tempdir().unwrap();
        crate::device_access::save(
            &root.path().join("executor.sqlite3"),
            &crate::device_access::DeviceAccess {
                tenant_id: "tenant".to_owned(),
                device_id: "device-id".to_owned(),
                device_code: "123456789".to_owned(),
                temporary_password: "current-password".to_owned(),
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
            root.path().join("executor-heartbeat.json"),
            json!({"observed_at_unix_ms":now,"control_phase":"Connected"}).to_string(),
        )
        .unwrap();
        let status = read_device_status(root.path()).await.unwrap();
        assert_eq!(status.device_code, "123456789");
        assert_eq!(status.temporary_password, "current-password");
        assert!(status.executor_running);
        assert_eq!(status.control_phase, "Connected");
    }

    #[tokio::test]
    async fn reads_saved_access_without_a_control_heartbeat() {
        let root = tempfile::tempdir().unwrap();
        crate::device_access::save(
            &root.path().join("executor.sqlite3"),
            &crate::device_access::DeviceAccess {
                tenant_id: "tenant".to_owned(),
                device_id: "device-id".to_owned(),
                device_code: "123456789".to_owned(),
                temporary_password: "ABCDEFGH".to_owned(),
                password_version: 1,
                password_hash: "hash".to_owned(),
            },
        )
        .await
        .unwrap();

        let status = read_device_status_from_service(root.path()).await.unwrap();
        assert_eq!(status.device_code, "123456789");
        assert_eq!(status.temporary_password, "ABCDEFGH");
        assert_eq!(status.control_phase, "Unknown");
        assert!(status.executor_running);
    }
}
