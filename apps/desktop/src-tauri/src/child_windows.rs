//! Child WebViews share the main window's environment, including after an
//! Evergreen Runtime update. COM references never leave the UI apartment.
use tauri::{AppHandle, Manager, WebviewWindow, WebviewWindowBuilder, Wry};

#[cfg(windows)]
thread_local! {
    static MAIN_ENVIRONMENT: std::cell::RefCell<Option<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment>> = const { std::cell::RefCell::new(None) };
}

pub fn initialize(app: &tauri::App) -> tauri::Result<()> {
    #[cfg(windows)]
    app.get_webview_window("main")
        .ok_or_else(|| tauri::Error::WindowNotFound)?
        .with_webview(|webview| {
            MAIN_ENVIRONMENT.with(|slot| *slot.borrow_mut() = Some(webview.environment()));
        })?;
    #[cfg(not(windows))]
    let _ = app;
    Ok(())
}

fn creation_error(error: tauri::Error) -> String {
    let detail = format!("{error:?}");
    tracing::warn!(%error, %detail, "child window creation failed");
    if detail.to_ascii_lowercase().contains("0x80010108")
        || detail.contains("RPC_E_DISCONNECTED")
        || detail.contains("object invoked has disconnected")
    {
        "webview_environment_disconnected".into()
    } else {
        error.to_string()
    }
}

/// The factory contains owned window options, not apartment-bound COM objects.
/// The main-thread task is separate from with_webview's borrowed window callback
/// so building a child cannot reenter Tauri while its window map is borrowed.
pub async fn build<F>(app: &AppHandle, factory: F) -> Result<WebviewWindow, String>
where
    F: for<'a> FnOnce(&'a AppHandle) -> Result<WebviewWindowBuilder<'a, Wry, AppHandle>, String>
        + Send
        + 'static,
{
    #[cfg(windows)]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let handle = app.clone();
        app.run_on_main_thread(move || {
            let environment = MAIN_ENVIRONMENT.with(|slot| slot.borrow().clone());
            let result = match environment {
                Some(environment) if handle.get_webview_window("main").is_some() => {
                    factory(&handle).and_then(|builder| {
                        builder
                            .with_environment(environment)
                            .build()
                            .map_err(creation_error)
                    })
                }
                _ => Err("webview_environment_unavailable".into()),
            };
            // A cancelled caller must not leave an unowned hidden child behind.
            if let Err(Ok(window)) = sender.send(result) {
                let _ = window.destroy();
            }
        })
        .map_err(|error| error.to_string())?;
        receiver
            .await
            .map_err(|_| "webview_environment_unavailable".to_owned())?
    }
    #[cfg(not(windows))]
    factory(app)?.build().map_err(creation_error)
}
