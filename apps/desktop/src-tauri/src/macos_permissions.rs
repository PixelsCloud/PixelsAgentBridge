use serde::Serialize;

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
pub fn open_macos_permission_settings(permission: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let pane = match permission.as_str() {
            "screen" => "Privacy_ScreenCapture",
            "accessibility" => "Privacy_Accessibility",
            _ => return Err("unknown permission".into()),
        };
        let status = std::process::Command::new("/usr/bin/open")
            .arg(format!(
                "x-apple.systempreferences:com.apple.preference.security?{pane}"
            ))
            .status()
            .map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err("could not open System Settings".into())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = permission;
        Err("macOS settings are unavailable on this platform".into())
    }
}
