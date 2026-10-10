use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use pab_agent_core::{DataPaths, DataScope, ensure_data_dir};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs::{File, OpenOptions}, io::{Read, Write}, path::PathBuf, sync::Arc, time::Duration};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::RwLock;

const RELEASE_PUBLIC_KEY: &str = "dOqSsbkPvJmGFblm/tH23cwbRmHjVhu6POF+WLmnKXk=";
const DEFAULT_SITE: &str = "https://agent.rgaa.vip";
const INSTALLER_VERSION: &str = match option_env!("PAB_INSTALLER_VERSION") {
    Some(value) => value,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateInfo {
    schema: u32,
    platform: String,
    architecture: String,
    channel: String,
    version: String,
    filename: String,
    size: u64,
    sha256: String,
    signature: String,
    notes_zh: String,
    notes_en: String,
    published_at: String,
    download_url: String,
}

#[derive(Default)]
struct Inner {
    available: Option<UpdateInfo>,
    verified_path: Option<PathBuf>,
}

#[derive(Clone, Default)]
pub struct UpdateState(Arc<RwLock<Inner>>);

fn platform() -> Result<(&'static str, &'static str), String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok(("windows", "x86_64")),
        ("macos", "aarch64") => Ok(("macos", "aarch64")),
        _ => Err("No installer is published for this platform".into()),
    }
}

fn parse_version(version: &str) -> Option<[u32; 3]> {
    let values: Vec<_> = version.split('.').map(str::parse::<u32>).collect();
    if values.len() != 3 || values.iter().any(Result::is_err) { return None; }
    let parts = [*values[0].as_ref().ok()?, *values[1].as_ref().ok()?, *values[2].as_ref().ok()?];
    (parts[1] < 100 && parts[2] < 100).then_some(parts)
}

fn canonical(info: &UpdateInfo) -> String {
    format!("PAB-RELEASE-V1\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
        info.platform, info.architecture, info.channel, info.version,
        info.filename, info.size, info.sha256)
}

fn validate(info: &UpdateInfo) -> Result<(), String> {
    let (platform, architecture) = platform()?;
    if info.schema != 1 || info.platform != platform || info.architecture != architecture || info.channel != "stable"
        || parse_version(&info.version) <= parse_version(INSTALLER_VERSION) || info.size == 0 || info.size > 5_000_000_000
        || info.sha256.len() != 64 || !info.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid update metadata".into());
    }
    let extension = if platform == "windows" { "exe" } else { "pkg" };
    if info.filename != format!("pixels-agent-bridge-{platform}-{architecture}-release-{}-setup.{extension}", info.version) {
        return Err("Update filename does not match this device".into());
    }
    let url = reqwest::Url::parse(&info.download_url).map_err(|_| "Invalid update URL")?;
    if url.scheme() != "https" || url.host_str().is_none_or(|h| !h.ends_with(".cos.ap-beijing.myqcloud.com"))
        || !url.path().starts_with("/releases/") || url.query().is_some() || url.fragment().is_some()
        || !url.username().is_empty() || url.password().is_some() {
        return Err("Untrusted update URL".into());
    }
    let key = STANDARD.decode(RELEASE_PUBLIC_KEY).map_err(|_| "Invalid embedded release key")?;
    let key = VerifyingKey::from_bytes(&key.try_into().map_err(|_| "Invalid embedded release key")?)
        .map_err(|_| "Invalid embedded release key")?;
    let signature = STANDARD.decode(&info.signature).map_err(|_| "Invalid update signature")?;
    let signature = Signature::try_from(signature.as_slice()).map_err(|_| "Invalid update signature")?;
    key.verify(canonical(info).as_bytes(), &signature).map_err(|_| "Update signature verification failed".into())
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(900)).build().map_err(|_| "Could not prepare update connection".into())
}

async fn query(state: &UpdateState) -> Result<Option<UpdateInfo>, String> {
    let (platform, architecture) = platform()?;
    let base = std::env::var("PAB_UPDATE_SITE_URL").unwrap_or_else(|_| DEFAULT_SITE.into());
    let endpoint = format!("{}/api/updates/check", base.trim_end_matches('/'));
    if !endpoint.starts_with("https://") { return Err("Update service must use HTTPS".into()); }
    let response = client()?.get(endpoint).query(&[
        ("platform", platform), ("architecture", architecture), ("channel", "stable"), ("current", INSTALLER_VERSION)
    ]).send().await.map_err(|_| "Could not reach update service")?;
    if response.status() == reqwest::StatusCode::NO_CONTENT {
        let mut inner = state.0.write().await;
        inner.available = None;
        inner.verified_path = None;
        return Ok(None);
    }
    if !response.status().is_success() { return Err("Update service returned an error".into()); }
    let info: UpdateInfo = response.json().await.map_err(|_| "Invalid update service response")?;
    validate(&info)?;
    let mut inner = state.0.write().await;
    inner.verified_path = None;
    inner.available = Some(info.clone());
    Ok(Some(info))
}

#[tauri::command]
pub async fn check_for_update(state: State<'_, UpdateState>) -> Result<Option<UpdateInfo>, String> {
    query(state.inner()).await
}

#[tauri::command]
pub async fn update_status(state: State<'_, UpdateState>) -> Result<Option<UpdateInfo>, String> {
    Ok(state.0.read().await.available.clone())
}

fn cache_path(filename: &str) -> Result<PathBuf, String> {
    let paths = DataPaths::for_scope(DataScope::User).map_err(|e| e.to_string())?;
    let directory = paths.root().join("updates");
    ensure_data_dir(&directory).map_err(|e| e.to_string())?;
    Ok(directory.join(filename))
}

fn verify_file(path: &PathBuf, info: &UpdateInfo) -> Result<(), String> {
    let mut file = File::open(path).map_err(|_| "Downloaded installer is missing")?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    let mut size = 0_u64;
    loop {
        let n = file.read(&mut buffer).map_err(|_| "Could not read downloaded installer")?;
        if n == 0 { break; }
        size += n as u64;
        digest.update(&buffer[..n]);
    }
    if size != info.size || format!("{:x}", digest.finalize()) != info.sha256 {
        return Err("Downloaded installer checksum does not match".into());
    }
    validate(info)
}

#[tauri::command]
pub async fn download_update(app: AppHandle, state: State<'_, UpdateState>) -> Result<(), String> {
    let info = state.0.read().await.available.clone().ok_or("No update is available")?;
    validate(&info)?;
    let path = cache_path(&info.filename)?;
    if path.is_file() && verify_file(&path, &info).is_ok() {
        state.0.write().await.verified_path = Some(path);
        return Ok(());
    }
    let partial = path.with_extension("part");
    let mut offset = partial.metadata().map(|m| m.len()).unwrap_or(0);
    if offset > info.size { std::fs::remove_file(&partial).map_err(|_| "Could not reset update download")?; offset = 0; }
    if offset == info.size {
        if verify_file(&partial, &info).is_ok() {
            std::fs::rename(&partial, &path).map_err(|_| "Could not finalize update download")?;
            state.0.write().await.verified_path = Some(path);
            return Ok(());
        }
        std::fs::remove_file(&partial).map_err(|_| "Could not reset damaged update download")?;
        offset = 0;
    }
    let mut request = client()?.get(&info.download_url);
    if offset > 0 { request = request.header(reqwest::header::RANGE, format!("bytes={offset}-")); }
    let mut response = request.send().await.map_err(|_| "Update download failed")?;
    if offset > 0 && response.status() == reqwest::StatusCode::OK { offset = 0; }
    if offset > 0 {
        let expected = format!("bytes {offset}-");
        if response.status() != reqwest::StatusCode::PARTIAL_CONTENT ||
            !response.headers().get(reqwest::header::CONTENT_RANGE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with(&expected)) {
            return Err("Update server did not honor resume request".into());
        }
    } else if response.status() != reqwest::StatusCode::OK {
        return Err("Update download returned an unexpected status".into());
    }
    let mut file = OpenOptions::new().create(true).write(true).append(offset > 0).truncate(offset == 0)
        .open(&partial).map_err(|_| "Could not open update cache")?;
    let mut received = offset;
    while let Some(chunk) = response.chunk().await.map_err(|_| "Update download interrupted")? {
        received += chunk.len() as u64;
        if received > info.size { return Err("Downloaded installer is larger than expected".into()); }
        file.write_all(&chunk).map_err(|_| "Could not write update cache")?;
        let _ = app.emit("update-download-progress", serde_json::json!({"received":received,"total":info.size}));
    }
    file.sync_all().map_err(|_| "Could not flush update cache")?;
    drop(file);
    if let Err(error) = verify_file(&partial, &info) {
        let _ = std::fs::remove_file(&partial);
        return Err(error);
    }
    std::fs::rename(&partial, &path).map_err(|_| "Could not finalize update download")?;
    state.0.write().await.verified_path = Some(path);
    Ok(())
}

#[cfg(windows)]
fn start_installer(path: &PathBuf) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let result = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), path.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
    if (result as usize) <= 32 { return Err("Could not start the installer".into()); }
    Ok(())
}

#[cfg(target_os = "macos")]
fn start_installer(path: &PathBuf) -> Result<(), String> {
    std::process::Command::new("/usr/bin/open").arg("-a").arg("Installer").arg(path)
        .spawn().map(|_| ()).map_err(|_| "Could not open the macOS installer".into())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn start_installer(_path: &PathBuf) -> Result<(), String> { Err("Installer not supported on this platform".into()) }

#[tauri::command]
pub async fn install_update(app: AppHandle, state: State<'_, UpdateState>) -> Result<(), String> {
    let inner = state.0.read().await;
    let info = inner.available.as_ref().ok_or("No update is available")?;
    let path = inner.verified_path.as_ref().ok_or("Download the installer first")?;
    verify_file(path, info)?;
    start_installer(path)?;
    app.exit(0);
    Ok(())
}

pub fn start_background_checks(app: AppHandle, state: UpdateState) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(15)).await;
        loop {
            if let Ok(Some(info)) = query(&state).await { let _ = app.emit("update-available", info); }
            tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_order_is_numeric() {
        assert!(parse_version("1.2.75") > parse_version("1.2.74"));
        assert!(parse_version("1.3.0") > parse_version("1.2.99"));
        assert!(parse_version("1.2.100").is_none());
    }
}
