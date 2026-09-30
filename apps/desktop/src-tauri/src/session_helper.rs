use std::time::Duration;

use pab_agent_core::{DataPaths, DataScope};
use pab_executor::local_ipc::{self, LocalEvent, LocalSocket};

use crate::{desktop_input, screenshot_session, window_session};

pub fn run() -> Result<(), String> {
    let paths = DataPaths::for_scope(DataScope::User).map_err(|error| error.to_string())?;
    let expected_desktop = std::env::args()
        .nth(2)
        .and_then(|argument| argument.strip_prefix("--desktop=").map(str::to_owned));
    let log_role = match expected_desktop.as_deref() {
        Some("Default") => "session-helper-default",
        Some("Winlogon") => "session-helper-winlogon",
        _ => "session-helper",
    };
    pab_logging::init(log_role, paths.root()).map_err(|error| error.to_string())?;
    tauri::async_runtime::block_on(run_forever(expected_desktop))
}

async fn run_forever(expected_desktop: Option<String>) -> Result<(), String> {
    loop {
        if !desktop_is_active(expected_desktop.as_deref()) {
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }
        match local_ipc::connect_local().await {
            Ok(mut socket) => {
                if let Err(error) = local_ipc::register_window_helper(&mut socket).await {
                    tracing::warn!(%error, "session helper registration failed");
                } else {
                    tracing::info!(
                        desktop = expected_desktop.as_deref().unwrap_or("unspecified"),
                        "session helper registered"
                    );
                    if let Err(error) =
                        serve_requests(&mut socket, expected_desktop.as_deref()).await
                    {
                        tracing::debug!(%error, "session helper disconnected");
                    }
                }
            }
            Err(error) => tracing::debug!(%error, "local Executor is unavailable"),
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn serve_requests(
    socket: &mut LocalSocket,
    expected_desktop: Option<&str>,
) -> Result<(), String> {
    loop {
        if !desktop_is_active(expected_desktop) {
            tracing::info!("session helper paused for desktop switch");
            return Ok(());
        }
        let event =
            match tokio::time::timeout(Duration::from_secs(1), local_ipc::next_local_event(socket))
                .await
            {
                Ok(result) => result.map_err(|error| error.to_string())?,
                Err(_) => continue,
            };
        match event {
            LocalEvent::Status(_) | LocalEvent::StatusUnavailable(_) => {}
            LocalEvent::ListWindows => {
                let entries = window_session::list_windows();
                local_ipc::reply_window_list(socket, &entries)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            LocalEvent::CaptureScreenshot => {
                let reply = match screenshot_session::capture_png() {
                    Ok(bytes) => local_ipc::reply_screenshot(socket, &bytes).await,
                    Err(message) => local_ipc::reply_screenshot_error(socket, &message).await,
                };
                reply.map_err(|error| error.to_string())?;
            }
            LocalEvent::CaptureScreenshotV2(options) => {
                let result = if desktop_is_active(expected_desktop) {
                    screenshot_session::capture(&options)
                } else {
                    Err("interactive desktop changed".into())
                };
                let reply = match result {
                    Ok(image) if desktop_is_active(expected_desktop) => {
                        local_ipc::reply_screenshot_v2(socket, &image).await
                    }
                    Ok(_) => {
                        local_ipc::reply_screenshot_error(
                            socket,
                            "interactive desktop changed during capture",
                        )
                        .await
                    }
                    Err(message) => local_ipc::reply_screenshot_error(socket, &message).await,
                };
                reply.map_err(|e| e.to_string())?;
            }
            LocalEvent::DesktopInput(event) => {
                let result = if desktop_is_active(expected_desktop) {
                    desktop_input::apply(event)
                } else {
                    Err("interactive desktop changed".to_owned())
                };
                local_ipc::reply_desktop_input(
                    socket,
                    result.as_ref().map(|_| ()).map_err(String::as_str),
                )
                .await
                .map_err(|error| error.to_string())?;
            }
        }
    }
}

#[cfg(windows)]
pub(crate) fn desktop_is_active(expected: Option<&str>) -> bool {
    use windows_sys::Win32::System::StationsAndDesktops::{
        CloseDesktop, DESKTOP_READOBJECTS, GetUserObjectInformationW, OpenInputDesktop, UOI_NAME,
    };

    let Some(expected) = expected else {
        return true;
    };
    // SAFETY: The returned desktop handle is closed before this function returns.
    let desktop = unsafe { OpenInputDesktop(0, 0, DESKTOP_READOBJECTS) };
    if desktop.is_null() {
        return false;
    }
    let mut name = [0u16; 128];
    let mut needed = 0u32;
    // SAFETY: name is a writable UTF-16 buffer, and desktop is a valid handle.
    let success = unsafe {
        GetUserObjectInformationW(
            desktop,
            UOI_NAME,
            name.as_mut_ptr().cast(),
            std::mem::size_of_val(&name) as u32,
            &mut needed,
        )
    } != 0;
    // SAFETY: OpenInputDesktop returned this handle.
    unsafe { CloseDesktop(desktop) };
    if !success {
        return false;
    }
    let length = name
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(name.len());
    String::from_utf16_lossy(&name[..length]).eq_ignore_ascii_case(expected)
}

#[cfg(not(windows))]
pub(crate) fn desktop_is_active(_expected: Option<&str>) -> bool {
    true
}
