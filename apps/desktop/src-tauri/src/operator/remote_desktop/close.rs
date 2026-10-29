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
    // The owned app dialog remains above native video and collects input locally.
    if !crate::confirmation::ask(window.app_handle(), Some(window), title, body, confirm, cancel).await {
        return Ok(false);
    }
    state.close(window.label()).await;
    // Destroy bypasses CloseRequested, which has already been confirmed.
    window.destroy().map_err(|error| error.to_string())?;
    Ok(true)
}
