use serde::Serialize;
use std::{io::Read, path::PathBuf};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRuntimeInfo {
    ready: bool,
    chrome_available: bool,
    edge_available: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileStatus {
    signed_in: bool,
    user_id: Option<String>,
    chrome_saved: bool,
    edge_saved: bool,
}

#[tauri::command]
pub fn browser_profile_status() -> Result<BrowserProfileStatus, String> {
    let store = pab_agent_core::account::AccountStore::from_env().map_err(|e| e.to_string())?;
    let state = store.read().map_err(|e| e.to_string())?;
    let Some(user) = state.user else {
        return Ok(BrowserProfileStatus {
            signed_in: false,
            user_id: None,
            chrome_saved: false,
            edge_saved: false,
        });
    };
    let root = pab_agent_core::DataPaths::for_scope(pab_agent_core::DataScope::User)
        .map_err(|e| e.to_string())?;
    let [chrome_saved, edge_saved] = crate::browser_helper::saved_profiles(root.root(), user.id);
    Ok(BrowserProfileStatus {
        signed_in: true,
        user_id: Some(user.id.to_string()),
        chrome_saved,
        edge_saved,
    })
}

#[tauri::command]
pub async fn browser_profile_clear(
    state: tauri::State<'_, crate::operator::OperatorState>,
    browser: String,
    expected_user_id: String,
) -> Result<BrowserProfileStatus, String> {
    let _guard = state.account_changes.lock().await;
    let store = pab_agent_core::account::AccountStore::from_env().map_err(|e| e.to_string())?;
    let state = store.read().map_err(|e| e.to_string())?;
    let user = state
        .user
        .ok_or("pixels_login_required: sign in before clearing browser data")?;
    if user.id.to_string() != expected_user_id {
        return Err("pixels_account_changed: refresh browser settings before clearing data".into());
    }
    let root = pab_agent_core::DataPaths::for_scope(pab_agent_core::DataScope::User)
        .map_err(|e| e.to_string())?;
    crate::browser_helper::clear_profile(root.root(), user.id, &browser)?;
    browser_profile_status()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserHandoffStatus {
    reason: String,
    expires_at_unix_ms: i64,
    ready: bool,
    cancelled: bool,
}

fn read_handoff() -> Result<Option<(serde_json::Value, PathBuf)>, String> {
    let path = crate::browser_helper::handoff_path()?;
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 4096 {
        return Err("invalid browser handoff marker".into());
    }
    let marker: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "invalid browser handoff marker")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    if marker["expires_at_unix_ms"]
        .as_i64()
        .is_none_or(|at| at <= now)
    {
        return Ok(None);
    }
    if marker["session_ref"].as_str().is_none()
        || marker["reason"].as_str().is_none()
        || !marker["ready"].is_boolean()
        || !marker["cancelled"].is_boolean()
    {
        return Err("invalid browser handoff marker".into());
    }
    if marker["helper_pid"].as_u64().is_some_and(|pid| {
        pid > u32::MAX as u64 || !helper_process_alive(pid as u32)
    }) {
        return Ok(None);
    }
    Ok(Some((marker, path)))
}

#[cfg(windows)]
fn helper_process_alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    if pid == 0 { return false; }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() { return false; }
    let mut exit_code = 0;
    let alive = unsafe { GetExitCodeProcess(process, &mut exit_code) != 0 && exit_code == 259 };
    unsafe { CloseHandle(process) };
    alive
}

#[cfg(target_os = "macos")]
fn helper_process_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 { return false; }
    unsafe { libc::kill(pid as i32, 0) == 0 || *libc::__error() == libc::EPERM }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn helper_process_alive(_pid: u32) -> bool { true }

pub(crate) fn pending_handoff_session_ref() -> Option<String> {
    let (marker, _) = read_handoff().ok()??;
    if marker["ready"] == true || marker["cancelled"] == true {
        return None;
    }
    marker["session_ref"].as_str().map(str::to_owned)
}

#[tauri::command]
pub fn browser_handoff_status() -> Result<Option<BrowserHandoffStatus>, String> {
    let Some((marker, _)) = read_handoff()? else {
        return Ok(None);
    };
    Ok(Some(BrowserHandoffStatus {
        reason: marker["reason"].as_str().unwrap_or("").to_owned(),
        expires_at_unix_ms: marker["expires_at_unix_ms"].as_i64().unwrap_or(0),
        ready: marker["ready"] == true,
        cancelled: marker["cancelled"] == true,
    }))
}

#[tauri::command]
pub fn browser_handoff_complete() -> Result<BrowserHandoffStatus, String> {
    let Some((mut marker, path)) = read_handoff()? else {
        return Err("browser handoff has expired".into());
    };
    if marker["cancelled"] == true {
        return Err("browser handoff was cancelled".into());
    }
    marker["ready"] = serde_json::Value::Bool(true);
    write_handoff_marker(&path, &marker)?;
    Ok(BrowserHandoffStatus {
        reason: marker["reason"].as_str().unwrap_or("").to_owned(),
        expires_at_unix_ms: marker["expires_at_unix_ms"].as_i64().unwrap_or(0),
        ready: true,
        cancelled: false,
    })
}

#[tauri::command]
pub fn browser_handoff_cancel() -> Result<BrowserHandoffStatus, String> {
    let Some((mut marker, path)) = read_handoff()? else {
        return Err("browser handoff has expired".into());
    };
    marker["cancelled"] = serde_json::Value::Bool(true);
    marker["ready"] = serde_json::Value::Bool(false);
    write_handoff_marker(&path, &marker)?;
    Ok(BrowserHandoffStatus {
        reason: marker["reason"].as_str().unwrap_or("").to_owned(),
        expires_at_unix_ms: marker["expires_at_unix_ms"].as_i64().unwrap_or(0),
        ready: false,
        cancelled: true,
    })
}

fn write_handoff_marker(path: &std::path::Path, marker: &serde_json::Value) -> Result<(), String> {
    let parent = path.parent().ok_or("invalid browser handoff path")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer(file.as_file_mut(), marker).map_err(|e| e.to_string())?;
    file.as_file_mut().sync_all().map_err(|e| e.to_string())?;
    pab_agent_core::restrict_private_file(file.path()).map_err(|e| e.to_string())?;
    file.persist(&path).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn browser_runtime_info() -> Result<BrowserRuntimeInfo, String> {
    let root = crate::browser_helper::packaged_runtime()?;
    let node = root.join(if cfg!(windows) { "node.exe" } else { "node" });
    let ready = node.is_file() && root.join("node_modules/@playwright/mcp/cli.js").is_file();
    Ok(BrowserRuntimeInfo {
        ready,
        chrome_available: crate::browser_helper::chrome_executable().is_some(),
        edge_available: crate::browser_helper::edge_executable().is_some(),
    })
}
