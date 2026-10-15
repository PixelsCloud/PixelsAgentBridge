use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionTarget {
    Desktop,
    Executor,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionTargetInfo {
    target: PermissionTarget,
    path: String,
    exists: bool,
}

fn permission_pane(permission: &str) -> Result<&'static str, String> {
    match permission {
        "screen" => Ok("Privacy_ScreenCapture"),
        "accessibility" => Ok("Privacy_Accessibility"),
        "full_disk" => Ok("Privacy_AllFiles"),
        _ => Err("unknown permission".into()),
    }
}

fn main_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("open permission settings from the main Pixels window".into())
    }
}

#[cfg(target_os = "macos")]
fn permission_target_path(target: PermissionTarget) -> Result<std::path::PathBuf, String> {
    match target {
        PermissionTarget::Executor => Ok(std::path::PathBuf::from(
            "/Library/Application Support/PixelsAgentBridge/pab-executor",
        )),
        PermissionTarget::Desktop => {
            let executable = std::env::current_exe().map_err(|e| e.to_string())?;
            // The installed app may be moved. Reveal this running bundle rather
            // than assuming that an older copy in /Applications is the right one.
            if let Some(bundle) = executable.ancestors().find(|path| {
                path.extension().is_some_and(|ext| ext == "app")
                    && path.join("Contents/Info.plist").is_file()
            }) {
                return Ok(bundle.to_path_buf());
            }
            // Standalone development binaries have no grantable app bundle.
            // Explain the installed application's location instead.
            Ok(std::path::PathBuf::from(
                "/Applications/Pixels Agent Bridge.app",
            ))
        }
    }
}

#[tauri::command]
pub fn macos_permission_targets(
    window: tauri::WebviewWindow,
) -> Result<Vec<PermissionTargetInfo>, String> {
    main_window(&window)?;
    #[cfg(target_os = "macos")]
    {
        [PermissionTarget::Desktop, PermissionTarget::Executor]
            .into_iter()
            .map(|target| {
                let path = permission_target_path(target)?;
                Ok(PermissionTargetInfo {
                    target,
                    exists: path.exists(),
                    path: path.to_string_lossy().into_owned(),
                })
            })
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(Vec::new())
    }
}

#[tauri::command]
pub async fn macos_file_access_check(
    window: tauri::WebviewWindow,
    target: PermissionTarget,
) -> Result<pab_executor::file_access::FileAccessReport, String> {
    main_window(&window)?;
    #[cfg(target_os = "macos")]
    {
        match target {
            PermissionTarget::Desktop => {
                let caller = pab_executor::file_access::current_user_home()?;
                let executable = std::env::current_exe().map_err(|e| e.to_string())?;
                pab_executor::file_access::check(executable, caller).await
            }
            PermissionTarget::Executor => pab_executor::local_ipc::check_local_file_access().await.map_err(|e| e.to_string()),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = target;
        Err("macOS file access checks are unavailable on this platform".into())
    }
}

#[tauri::command]
pub async fn reveal_macos_permission_target(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    target: PermissionTarget,
) -> Result<(), String> {
    main_window(&window)?;
    #[cfg(target_os = "macos")]
    {
        let path = permission_target_path(target)?;
        if !path.exists() {
            return Err(format!("Pixels program was not found: {}", path.display()));
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            use objc2_app_kit::NSWorkspace;
            use objc2_foundation::{NSArray, NSString, NSURL};
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            NSWorkspace::sharedWorkspace()
                .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
            let _ = sender.send(());
        })
        .map_err(|e| e.to_string())?;
        receiver
            .await
            .map_err(|_| "opening Finder interrupted".into())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, target);
        Err("macOS settings are unavailable on this platform".into())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    screen_recording: bool,
    accessibility: bool,
}

#[tauri::command]
pub fn macos_permissions() -> Option<Permissions> {
    #[cfg(target_os = "macos")]
    {
        Some(Permissions {
            screen_recording: pab_desktop_control::screen_capture_allowed(),
            accessibility: pab_desktop_control::accessibility_allowed(),
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[tauri::command]
pub async fn request_macos_permission(
    app: tauri::AppHandle,
    permission: String,
    automatic: bool,
) -> Result<Option<Permissions>, String> {
    #[cfg(target_os = "macos")]
    {
        use std::sync::atomic::{AtomicU8, Ordering};
        static REQUESTED: AtomicU8 = AtomicU8::new(0);
        let bit = match permission.as_str() {
            "screen" => 1,
            "accessibility" => 2,
            _ => return Err("unknown permission".into()),
        };
        // React StrictMode/remounts must not repeat native prompts. Explicit retry is allowed.
        if automatic && REQUESTED.fetch_or(bit, Ordering::SeqCst) & bit != 0 {
            return Ok(macos_permissions());
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            let result = match permission.as_str() {
                "screen" => {
                    pab_desktop_control::request_screen_capture();
                    Ok(())
                }
                _ => pab_desktop_control::request_accessibility().map(|_| ()),
            };
            let _ = sender.send(result.map(|_| macos_permissions()));
        })
        .map_err(|e| e.to_string())?;
        receiver
            .await
            .map_err(|_| "permission request interrupted".to_owned())?
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, permission, automatic);
        Ok(None)
    }
}

#[tauri::command]
pub async fn open_macos_permission_settings(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    permission: String,
) -> Result<(), String> {
    main_window(&window)?;
    let pane = permission_pane(&permission)?;
    #[cfg(target_os = "macos")]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            use objc2_app_kit::NSWorkspace;
            use objc2_foundation::{NSString, NSURL};
            let url = NSURL::URLWithString(&NSString::from_str(&format!(
                "x-apple.systempreferences:com.apple.preference.security?{pane}"
            )));
            let opened = url.is_some_and(|url| NSWorkspace::sharedWorkspace().openURL(&url));
            let _ = sender.send(if opened {
                Ok(())
            } else {
                Err("could not open System Settings".to_owned())
            });
        })
        .map_err(|e| e.to_string())?;
        receiver
            .await
            .map_err(|_| "opening System Settings interrupted".to_owned())?
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, permission, pane);
        Err("macOS settings are unavailable on this platform".into())
    }
}

/// A fixed, user-triggered service operation. macOS owns the administrator
/// confirmation; Pixels never reads a password or edits the TCC database.
#[tauri::command]
pub async fn restart_macos_executor_for_permissions(
    window: tauri::WebviewWindow,
) -> Result<(), String> {
    main_window(&window)?;
    #[cfg(target_os = "macos")]
    {
        let output = tokio::process::Command::new("/usr/bin/osascript")
            .args([
                "-e",
                "do shell script \"/bin/launchctl kickstart -k system/com.pixelsagentbridge.executor\" with administrator privileges",
            ])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "Background service restart was not completed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("macOS permissions are unavailable on this platform".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_privacy_pages_are_accepted() {
        assert_eq!(permission_pane("screen").unwrap(), "Privacy_ScreenCapture");
        assert_eq!(
            permission_pane("accessibility").unwrap(),
            "Privacy_Accessibility"
        );
        assert_eq!(permission_pane("full_disk").unwrap(), "Privacy_AllFiles");
        assert!(permission_pane("full_disk; arbitrary command").is_err());
    }

    #[test]
    fn permission_targets_cannot_be_arbitrary_paths() {
        assert!(serde_json::from_str::<PermissionTarget>("\"desktop\"").is_ok());
        assert!(serde_json::from_str::<PermissionTarget>("\"executor\"").is_ok());
        assert!(serde_json::from_str::<PermissionTarget>("\"/tmp/other-program\"").is_err());
    }
}

/// Explicit user action only: apply newly granted permissions to both processes.
#[tauri::command]
pub async fn restart_macos_permission_processes(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn getuid() -> u32;
        }
        let service = format!("gui/{}/com.pixelsagentbridge.session-helper", unsafe {
            getuid()
        });
        let result = tokio::process::Command::new("/bin/launchctl")
            .args(["kill", "SIGTERM", &service])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        if !result.status.success() {
            return Err(
                "Could not restart the desktop helper; check that Pixels Agent Bridge is installed"
                    .into(),
            );
        }
        pab_desktop_control::release_input();
        app.restart();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("macOS permissions are unavailable on this platform".into())
    }
}
