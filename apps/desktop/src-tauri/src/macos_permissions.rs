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
pub async fn open_macos_permission_settings(
    app: tauri::AppHandle,
    permission: String,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let pane = match permission.as_str() {
            "screen" => "Privacy_ScreenCapture",
            "accessibility" => "Privacy_Accessibility",
            _ => return Err("unknown permission".into()),
        };
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
        let _ = (app, permission);
        Err("macOS settings are unavailable on this platform".into())
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
