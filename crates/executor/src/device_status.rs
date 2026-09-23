use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
use std::{
    env,
    os::windows::process::CommandExt,
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use pab_agent_core::{DataPaths, DataScope};
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Serialize)]
pub struct DeviceStatus {
    pub device_code: String,
    pub device_id: String,
    pub temporary_password: String,
    pub executor_running: bool,
    pub control_phase: String,
}

pub fn read_local_device_status() -> Result<DeviceStatus, DeviceStatusError> {
    let paths = DataPaths::for_scope(DataScope::Machine)?;
    read_device_status(paths.root())
}

fn read_device_status(root: &Path) -> Result<DeviceStatus, DeviceStatusError> {
    let info: Value = serde_json::from_slice(&fs::read(root.join("device-info.json"))?)?;
    let field = |name: &'static str| -> Result<String, DeviceStatusError> {
        info.get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(DeviceStatusError::MissingField(name))
    };
    let password = fs::read_to_string(root.join("current-password.txt"))?;
    let heartbeat: Option<Value> = fs::read(root.join("executor-heartbeat.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let control_phase = heartbeat
        .as_ref()
        .and_then(|value| value.get("control_phase"))
        .and_then(Value::as_str)
        .unwrap_or("Unknown")
        .to_owned();
    let observed = heartbeat
        .as_ref()
        .and_then(|value| value.get("observed_at_unix_ms"))
        .and_then(Value::as_u64);
    let now = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())
        .map_err(|_| DeviceStatusError::ClockOverflow)?;
    let heartbeat_running = observed
        .map(|observed| now.saturating_sub(observed) <= 10_000)
        .unwrap_or(false);
    Ok(DeviceStatus {
        device_code: field("device_code")?,
        device_id: field("device_id")?,
        temporary_password: password.trim().to_owned(),
        executor_running: heartbeat_running || service_running(),
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

    let task_name = env::var("PAB_EXECUTOR_TASK_NAME")
        .unwrap_or_else(|_| "PixelsAgentBridgeExecutor".to_owned());
    let escaped_name = task_name.replace('\'', "''");
    let command = format!(
        "(Get-ScheduledTask -TaskName '{escaped_name}' -ErrorAction Stop).State.ToString()"
    );
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let running = Command::new("powershell.exe")
        .creation_flags(CREATE_NO_WINDOW)
        .args(["-NoProfile", "-NonInteractive", "-Command", &command])
        .output()
        .map(|output| output.status.success() && output.stdout.trim_ascii() == b"Running")
        .unwrap_or(false);
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

    #[test]
    fn reads_device_access_and_live_status() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("device-info.json"),
            json!({"device_code":"123456789","device_id":"device-id"}).to_string(),
        )
        .unwrap();
        fs::write(
            root.path().join("current-password.txt"),
            "current-password\n",
        )
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
        let status = read_device_status(root.path()).unwrap();
        assert_eq!(status.device_code, "123456789");
        assert_eq!(status.temporary_password, "current-password");
        assert!(status.executor_running);
        assert_eq!(status.control_phase, "Connected");
    }
}
