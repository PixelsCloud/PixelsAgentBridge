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
