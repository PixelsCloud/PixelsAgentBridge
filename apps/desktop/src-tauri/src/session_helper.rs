use std::time::Duration;

use pab_agent_core::{DataPaths, DataScope};
use pab_executor::local_ipc::{self, LocalEvent, LocalSocket};

use crate::{desktop_input, screenshot_session, window_session};

#[path = "application_deadline.rs"]
mod application_deadline;

pub fn run() -> Result<(), String> {
    let paths = DataPaths::for_scope(DataScope::User).map_err(|error| error.to_string())?;
    let expected_desktop = std::env::args()
        .nth(2)
        .and_then(|argument| argument.strip_prefix("--desktop=").map(str::to_owned));
    let applications_only = std::env::args().nth(1).as_deref() == Some("--application-helper");
    if applications_only && (!cfg!(windows) || expected_desktop.as_deref() != Some("Default")) {
        return Err("application helper requires the Windows Default desktop".into());
    }
    let log_role = if applications_only {
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
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
        let main = MainThreadMarker::new().ok_or("session helper must start on the main thread")?;
        let app = NSApplication::sharedApplication(main);
        // A helper must service AppKit/AX and the main dispatch queue without
        // becoming the foreground app and taking focus away from its target.
        if !app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited) {
            return Err("unable to configure background session helper".into());
        }
        let task = tauri::async_runtime::spawn(async move {
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
            pab_desktop_control::release_input();
            std::process::exit(0);
        });
        app.run();
        task.abort();
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
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

async fn serve_application_requests(socket: &mut LocalSocket) -> Result<(), String> {
    // This helper never acquires input state or initializes UI automation.
    while application_desktop_is_active() {
        let event =
            match tokio::time::timeout(Duration::from_secs(1), local_ipc::next_local_event(socket))
                .await
            {
                Ok(result) => result.map_err(|e| e.to_string())?,
                Err(_) => continue,
            };
        match event {
            LocalEvent::Status(_) | LocalEvent::StatusUnavailable(_) => {}
            LocalEvent::ApplicationQuery(id, query, identity) => {
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
                Err(_) => continue,
            };
        match event {
            LocalEvent::Status(_) | LocalEvent::StatusUnavailable(_) => {}
            LocalEvent::ReleaseUiConnection(connection) => {
                desktop_session.release_ui_connection(connection)
            }
            LocalEvent::ApplicationQuery(id, query, identity) => {
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
