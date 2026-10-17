use std::time::Duration;

use pab_agent_core::{DataPaths, DataScope};
use pab_executor::local_ipc::{self, LocalEvent, LocalSocket};

use crate::{desktop_input, screenshot_session, window_session};

#[path = "application_deadline.rs"]
mod application_deadline;
#[cfg(target_os = "macos")]
#[path = "session_helper_macos_media.rs"]
mod macos_media;
#[cfg(target_os = "linux")]
#[path = "session_helper_linux_media.rs"]
pub(crate) mod linux_media;

pub fn run() -> Result<(), String> {
    let paths = DataPaths::for_scope(DataScope::User).map_err(|error| error.to_string())?;
    let expected_desktop = std::env::args()
        .nth(2)
        .and_then(|argument| argument.strip_prefix("--desktop=").map(str::to_owned));
    let applications_only = std::env::args().nth(1).as_deref() == Some("--application-helper");
    let media_only = std::env::args().nth(1).as_deref() == Some("--media-helper");
    if media_only {
        let supported = (cfg!(windows)
            && matches!(expected_desktop.as_deref(), Some("Default" | "Winlogon")))
            || (cfg!(target_os = "linux") && expected_desktop.is_none());
        if !supported {
            return Err("media helper requires a supported interactive session".into());
        }
    }
    if applications_only && (!cfg!(windows) || expected_desktop.as_deref() != Some("Default")) {
        return Err("application helper requires the Windows Default desktop".into());
    }
    let log_role = if media_only {
        "media-helper"
    } else if applications_only {
        "application-helper"
    } else {
        match expected_desktop.as_deref() {
            Some("Default") => "session-helper-default",
            Some("Winlogon") => "session-helper-winlogon",
            Some("LoginWindow") => "session-helper-loginwindow",
            _ => "session-helper",
        }
    };
    pab_logging::init(log_role, paths.root()).map_err(|error| error.to_string())?;
    #[cfg(windows)]
    if media_only {
        return tauri::async_runtime::block_on(async {
            let _worker = start_media_helper(expected_desktop)?;
            std::future::pending::<()>().await;
            Ok(())
        });
    }
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
        let main = MainThreadMarker::new().ok_or("session helper must start on the main thread")?;
        let app = NSApplication::sharedApplication(main);
        // A helper must service AppKit/AX and the main dispatch queue without
        // becoming the foreground app and taking focus away from its target.
        // An unbundled helper can already be Prohibited. AppKit may return
        // false for that no-op, so do not mistake it for a startup failure.
        if app.activationPolicy() != NSApplicationActivationPolicy::Prohibited
            && !app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited)
        {
            return Err("unable to configure background session helper".into());
        }
        // Finder can send the shared bundle's open event to this helper. Keep
        // LoginWindow headless; only an Aqua helper may forward to the GUI.
        let delegate = (expected_desktop.as_deref() != Some("LoginWindow"))
            .then(|| crate::macos_launch::HelperDelegate::new(main));
        if let Some(delegate) = delegate.as_ref() {
            app.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&**delegate)));
        }
        let task = tauri::async_runtime::spawn(async move {
            // A separate helper socket carries media/input; existing application
            // requests and their user identity keep their original route.
            let Ok(mut media) = macos_media::start(expected_desktop.clone()) else {
                tracing::error!("unable to start Mac media helper");
                std::process::exit(1);
            };
            use tokio::signal::unix::{SignalKind, signal};
            let Ok(mut termination) = signal(SignalKind::terminate()) else {
                tracing::error!("unable to install helper termination handler");
                std::process::exit(1);
            };
            tokio::select! {
                result = run_forever(expected_desktop, applications_only) => {
                    if let Err(error) = result { tracing::error!(%error, "session helper stopped"); }
                }
                _ = termination.recv() => {}
            }
            // The request loop has been dropped. Release on the still-running
            // AppKit thread before launchd starts the replacement helper.
            media.shutdown().await;
            pab_desktop_control::release_input();
            std::process::exit(0);
        });
        app.run();
        app.setDelegate(None);
        drop(delegate);
        task.abort();
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    return tauri::async_runtime::block_on(async {
        let mut media = if media_only { linux_media::start_media_only()? } else { linux_media::start()? };
        use tokio::signal::unix::{SignalKind, signal};
        let mut termination = signal(SignalKind::terminate()).map_err(|e| e.to_string())?;
        let result = tokio::select! {
            result = async {
                if media_only { std::future::pending::<Result<(), String>>().await }
                else { run_forever(expected_desktop, applications_only).await }
            } => result,
            _ = termination.recv() => Ok(()),
        };
        media.shutdown().await;
        result
    });
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    tauri::async_runtime::block_on(run_forever(expected_desktop, applications_only))
}

pub(crate) async fn register_desktop_helper(
    socket: &mut LocalSocket,
) -> Result<(), local_ipc::LocalIpcError> {
    if cfg!(target_os = "macos") {
        local_ipc::register_application_helper(socket).await
    } else {
        local_ipc::register_window_helper(socket).await
    }
}

async fn run_forever(
    expected_desktop: Option<String>,
    applications_only: bool,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let _recovery = crate::desktop_input::start_recovery_monitor();
    loop {
        if !(if applications_only {
            application_desktop_is_active()
        } else {
            desktop_is_active(expected_desktop.as_deref())
        }) {
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }
        match local_ipc::connect_local().await {
            Ok(mut socket) => {
                let registration = if applications_only {
                    local_ipc::register_application_only_helper(&mut socket).await
                } else {
                    register_desktop_helper(&mut socket).await
                };
                if let Err(error) = registration {
                    tracing::warn!(%error, "session helper registration failed");
                } else {
                    tracing::info!(
                        desktop = expected_desktop.as_deref().unwrap_or("unspecified"),
                        "session helper registered"
                    );
                    let result = if applications_only {
                        serve_application_requests(&mut socket).await
                    } else {
                        serve_requests(&mut socket, expected_desktop.as_deref()).await
                    };
                    if let Err(error) = result {
                        tracing::debug!(%error, "session helper disconnected");
                    }
                }
            }
            Err(error) => tracing::debug!(%error, "local Executor is unavailable"),
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[cfg(windows)]
struct MediaHelperTask(tauri::async_runtime::JoinHandle<()>);
#[cfg(windows)]
impl Drop for MediaHelperTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
#[cfg(windows)]
fn start_media_helper(desktop: Option<String>) -> Result<MediaHelperTask, String> {
    let backend = pab_desktop_media::NativeBackend::new(
        std::env::current_exe().map_err(|e| e.to_string())?,
        vec!["--media-worker".into()],
    )
    .with_input();
    Ok(MediaHelperTask(tauri::async_runtime::spawn(async move {
        loop {
            if desktop_is_active(desktop.as_deref()) {
                if let Err(error) = backend.recover_input().await {
                    tracing::debug!(%error, "waiting for previous input owner to release");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                let stopped = async {
                    loop {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        if !desktop_is_active(desktop.as_deref()) {
                            break;
                        }
                    }
                };
                tokio::select! {
                    result = local_ipc::media::serve_helper(&backend) => {
                        if let Err(error) = result { tracing::debug!(%error, "media helper disconnected"); }
                    }
                    _ = stopped => {}
                }
                // Do not re-register until the previous worker has really
                // exited; this gate also covers interrupted native startup.
                backend.idle().await;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })))
}

async fn serve_application_requests(socket: &mut LocalSocket) -> Result<(), String> {
    // This helper never acquires input state or initializes UI automation.
    let mut browser = crate::browser_helper::BrowserManager::default();
    while application_desktop_is_active() {
        let event =
            match tokio::time::timeout(Duration::from_secs(1), local_ipc::next_local_event(socket))
                .await
            {
                Ok(result) => result.map_err(|e| e.to_string())?,
                Err(_) => {
                    browser.expire_idle().await;
                    continue;
                }
            };
        match event {
            LocalEvent::Status(_) | LocalEvent::StatusUnavailable(_) => {}
            LocalEvent::ApplicationQuery(id, query, identity) => {
                if let pab_protocol::AppQuery::Browser { request } = &query {
                    let reply = browser
                        .query(id, request, &identity, application_desktop_is_active)
                        .await;
                    local_ipc::reply_desktop_query(socket, &reply)
                        .await
                        .map_err(|e| e.to_string())?;
                    continue;
                }
                // Shell extensions can block inside a native call indefinitely.
                // This dedicated process owns no input state and its children
                // are not in a kill-on-close job. Retire just this helper on a
                // deadline; the parent records an unconfirmed result, without
                // replaying the request or terminating a launched application.
                let reply = application_deadline::run(id, move || {
                    pab_desktop_control::apps::query_guarded(id, &query, &identity, || {
                        application_desktop_is_active()
                    })
                })
                .await?;
                local_ipc::reply_desktop_query(socket, &reply)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            _ => return Err("non-application request sent to application helper".into()),
        }
    }
    Ok(())
}

fn application_desktop_is_active() -> bool {
    #[cfg(windows)]
    {
        crate::windows_session_supervisor::current_application_session_is_active()
            && desktop_is_active(Some("Default"))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

async fn serve_requests(
    socket: &mut LocalSocket,
    expected_desktop: Option<&str>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let _input_guard = desktop_input::InputGuard;
    let mut desktop_session = pab_desktop_control::DesktopSession::new();
    let mut browser = crate::browser_helper::BrowserManager::default();
    loop {
        #[cfg(target_os = "macos")]
        pab_desktop_control::release_idle_input();
        if !desktop_is_active(expected_desktop) {
            tracing::info!("session helper paused for desktop switch");
            return Ok(());
        }
        let event =
            match tokio::time::timeout(Duration::from_secs(1), local_ipc::next_local_event(socket))
                .await
            {
                Ok(result) => result.map_err(|error| error.to_string())?,
                Err(_) => {
                    browser.expire_idle().await;
                    continue;
                }
            };
        match event {
            LocalEvent::Status(_) | LocalEvent::StatusUnavailable(_) => {}
            LocalEvent::ReleaseUiConnection(connection) => {
                desktop_session.release_ui_connection(connection)
            }
            LocalEvent::ApplicationQuery(id, query, identity) => {
                if let pab_protocol::AppQuery::Browser { request } = &query {
                    let reply = browser
                        .query(id, request, &identity, || {
                            desktop_is_active(expected_desktop)
                        })
                        .await;
                    local_ipc::reply_desktop_query(socket, &reply)
                        .await
                        .map_err(|e| e.to_string())?;
                    continue;
                }
                let reply = pab_desktop_control::apps::query_guarded(id, &query, &identity, || {
                    desktop_is_active(expected_desktop)
                });
                local_ipc::reply_desktop_query(socket, &reply)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            LocalEvent::DesktopQuery(id, query, context) => {
                let mut reply = if desktop_is_active(expected_desktop) {
                    desktop_session.query_guarded_context(id, &query, context, || {
                        if desktop_is_active(expected_desktop) {
                            Ok(())
                        } else {
                            Err(
                                "interactive desktop changed; remaining batch actions stopped"
                                    .into(),
                            )
                        }
                    })
                } else {
                    let mut reply = pab_protocol::SystemQueryReply::pending(
                        id,
                        &pab_protocol::SystemQuery::Desktop {
                            query: query.clone(),
                        },
                    );
                    reply.state = "failed".into();
                    reply.error = Some("interactive desktop changed".into());
                    reply
                };
                if !desktop_is_active(expected_desktop) {
                    reply.state = "unconfirmed".into();
                    reply.error = Some("interactive desktop changed during operation".into());
                }
                local_ipc::reply_desktop_query(socket, &reply)
                    .await
                    .map_err(|e| e.to_string())?;
            }
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
                    if options.window_ref.is_some() {
                        desktop_session.capture_window(&options)
                    } else {
                        screenshot_session::capture(&options)
                    }
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

#[cfg(target_os = "macos")]
pub(crate) fn desktop_is_active(expected: Option<&str>) -> bool {
    if expected == Some("LoginWindow") {
        pab_desktop_control::login_window_active()
    } else {
        pab_desktop_control::active_console() && !pab_desktop_control::login_window_active()
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn desktop_is_active(_expected: Option<&str>) -> bool {
    true
}
