use super::*;

/// This command resolves the caller's window rather than accepting a label.
/// Closing remains possible after logout or device removal invalidates its owner.
pub(super) async fn request(
    window: &WebviewWindow,
    state: &RemoteDesktopState,
    english: bool,
) -> Result<bool, String> {
    if !window.label().starts_with("desktop-viewer-") {
        return Err("only a remote desktop can close its own window".into());
    }
    let owner = state.windows.lock().await.get(window.label()).cloned();
    let Some(owner) = owner else {
        window.destroy().map_err(|error| error.to_string())?;
        return Ok(true);
    };
    let Ok(_request) = owner.close_request.try_lock() else {
        return Ok(false);
    };
    let (title, body, confirm, cancel) = if english {
        (
            "Close remote desktop?",
            "This will disconnect the current remote desktop session.",
            "Close connection",
            "Cancel",
        )
    } else {
        (
            "关闭远程桌面？",
            "关闭后将断开本次远程桌面连接。",
            "关闭连接",
            "取消",
        )
    };
    // The native video surface sits above the WebView. A parented system dialog
    // stays visible above it and keeps confirmation input on the local machine.
    let answer = rfd::AsyncMessageDialog::new()
        .set_parent(window)
        .set_title(title)
        .set_description(body)
        .set_level(rfd::MessageLevel::Warning)
        .set_buttons(rfd::MessageButtons::OkCancelCustom(
            confirm.into(),
            cancel.into(),
        ))
        .show()
        .await;
    if !matches!(answer, rfd::MessageDialogResult::Custom(label) if label == confirm) {
        return Ok(false);
    }
    state.close(window.label()).await;
    // Destroy bypasses CloseRequested, which has already been confirmed.
    window.destroy().map_err(|error| error.to_string())?;
    Ok(true)
}
