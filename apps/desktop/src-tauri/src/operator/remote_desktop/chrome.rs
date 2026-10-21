use super::*;

/// Local window state never travels through the remote desktop input protocol.
pub(super) fn set_fullscreen(
    window: &WebviewWindow,
    owner: &Owner,
    fullscreen: bool,
) -> Result<(), String> {
    if window.is_fullscreen().map_err(|error| error.to_string())? != fullscreen {
        #[cfg(windows)]
        if fullscreen {
            // Tao's undecorated WM_NCCALCSIZE path clamps a maximized window to
            // rcWork even in fullscreen. Restore first so rcMonitor is usable;
            // preserve the user's maximized state for leaving fullscreen.
            let maximized = window.is_maximized().map_err(|error| error.to_string())?;
            owner.restore_maximized.store(maximized, Ordering::Relaxed);
            if maximized {
                window.unmaximize().map_err(|error| error.to_string())?;
            }
        }
        if let Err(error) = window.set_fullscreen(fullscreen) {
            #[cfg(windows)]
            if fullscreen && owner.restore_maximized.swap(false, Ordering::Relaxed) {
                let _ = window.maximize();
            }
            return Err(error.to_string());
        }
        #[cfg(windows)]
        if !fullscreen && owner.restore_maximized.swap(false, Ordering::Relaxed) {
            window.maximize().map_err(|error| error.to_string())?;
        }
    }
    owner.fullscreen.send_replace(fullscreen);
    window
        .emit_to(window.label(), "desktop-viewer-fullscreen", fullscreen)
        .map_err(|error| error.to_string())
}
