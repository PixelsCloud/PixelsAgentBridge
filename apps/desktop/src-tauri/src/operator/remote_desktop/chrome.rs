use super::*;

/// Local window state never travels through the remote desktop input protocol.
pub(super) fn set_fullscreen(
    window: &WebviewWindow,
    owner: &Owner,
    fullscreen: bool,
) -> Result<(), String> {
    if window.is_fullscreen().map_err(|error| error.to_string())? != fullscreen {
        window
            .set_fullscreen(fullscreen)
            .map_err(|error| error.to_string())?;
    }
    owner.fullscreen.send_replace(fullscreen);
    window
        .emit_to(window.label(), "desktop-viewer-fullscreen", fullscreen)
        .map_err(|error| error.to_string())
}
