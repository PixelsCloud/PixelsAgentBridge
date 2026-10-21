use pab_agent_core::{DataPaths, DataScope};
use pab_executor::{DeviceStatus, read_local_device_status};
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tokio::sync::RwLock;

mod agent_integrations;
mod desktop_input;
#[cfg(target_os = "linux")]
mod linux_launch;
#[cfg(target_os = "linux")]
mod linux_media_supervisor;
#[cfg(target_os = "macos")]
mod macos_launch;
mod macos_permissions;
#[cfg(target_os = "macos")]
pub fn macos_diagnostics() -> serde_json::Value {
    serde_json::json!({
        "platform": "macos", "architecture": std::env::consts::ARCH,
        "activeConsole": pab_desktop_control::active_console(),
        "permissions": macos_permissions::macos_permissions(),
        "monitors": xcap::Monitor::all().map(|v| v.len()).map_err(|e| e.to_string()),
        "localAccessConfigured": pab_executor::local_ipc::user_token_path().is_ok_and(|p| p.is_file()),
    })
}
mod browser_helper;
mod browser_setup;
mod desktop_assistance;
mod device_account;
mod github_login;
mod mcp_reporting;
mod mcp_tool_settings;
mod operator;
mod screenshot_session;
mod server_settings;
mod session_helper;
mod tray;
mod update;
mod window_session;
#[cfg(windows)]
mod windows_gui;
#[cfg(windows)]
mod windows_session_supervisor;

#[derive(Clone, Default)]
struct LocalStatus(Arc<RwLock<Option<DeviceStatus>>>);

async fn show_pending_browser_handoff(handle: tauri::AppHandle) {
    let mut last_session = None;
    loop {
        let pending = browser_setup::pending_handoff_session_ref();
        if pending.is_some() && pending != last_session {
            tray::show_window(&handle);
        }
        last_session = pending;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

#[tauri::command]
async fn device_status(state: tauri::State<'_, LocalStatus>) -> Result<DeviceStatus, String> {
    if let Some(status) = state.0.read().await.clone() {
        return Ok(status);
    }
    read_local_device_status()
        .await
        .map_err(|error| error.to_string())
}

fn accept_local_status(current: &mut Option<DeviceStatus>, next: DeviceStatus) -> bool {
    if current.as_ref().is_some_and(|old| old.device_id == next.device_id && old.password_version > next.password_version) {
        return false;
    }
    *current = Some(next);
    true
}

#[tauri::command]
async fn rotate_device_password(app: tauri::AppHandle, state: tauri::State<'_, LocalStatus>) -> Result<DeviceStatus, String> {
    let status = pab_executor::local_ipc::rotate_local_password().await.map_err(|_| "password_refresh_failed")?;
    let mut current = state.0.write().await;
    accept_local_status(&mut current, status.clone());
    let status = current.clone().unwrap_or(status);
    let _ = app.emit("local-device-status", status.clone());
    Ok(status)
}

async fn watch_local_service(handle: tauri::AppHandle, status: LocalStatus) {
    #[cfg(target_os = "macos")]
    let _recovery = desktop_input::start_recovery_monitor();
    loop {
        match pab_executor::local_ipc::connect_local().await {
            Ok(mut socket) => {
                #[cfg(target_os = "macos")]
                let _input_guard = desktop_input::InputGuard;
                #[cfg(any(windows, target_os = "macos"))]
                if !session_helper::desktop_is_active(Some("Default")) {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
                #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
                if let Err(error) = session_helper::register_desktop_helper(&mut socket).await {
                    tracing::debug!(%error, "window helper registration failed");
                }
                let mut desktop_session = pab_desktop_control::DesktopSession::new();
                let mut browser = browser_helper::BrowserManager::default();
                loop {
                    #[cfg(target_os = "macos")]
                    pab_desktop_control::release_idle_input();
                    #[cfg(any(windows, target_os = "macos"))]
                    if !session_helper::desktop_is_active(Some("Default")) {
                        break;
                    }
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(1),
                        pab_executor::local_ipc::next_local_event(&mut socket),
                    )
                    .await
                    {
                        Ok(Ok(pab_executor::local_ipc::LocalEvent::Status(next))) => {
                            if accept_local_status(&mut *status.0.write().await, next.clone()) {
                                let _ = handle.emit("local-device-status", next);
                            }
                        }
                        Ok(Ok(pab_executor::local_ipc::LocalEvent::StatusUnavailable(message))) => {
                            tracing::debug!(%message, "local device status unavailable");
                            *status.0.write().await = None;
                            let _ = handle.emit("local-device-offline", ());
                        }
                        Ok(Ok(pab_executor::local_ipc::LocalEvent::ReleaseUiConnection(
                            connection,
                        ))) => {
                            desktop_session.release_ui_connection(connection);
                        }
                        Ok(Ok(pab_executor::local_ipc::LocalEvent::ApplicationQuery(
                            id,
                            query,
                            identity,
                        ))) => {
                            let reply = if let pab_protocol::AppQuery::Browser { request } = &query
                            {
                                browser
                                    .query(id, request, &identity, || {
                                        session_helper::desktop_is_active(Some("Default"))
                                    })
                                    .await
                            } else {
                                pab_desktop_control::apps::query_guarded(
                                    id,
                                    &query,
                                    &identity,
                                    || session_helper::desktop_is_active(Some("Default")),
                                )
                            };
                            if let Err(error) =
                                pab_executor::local_ipc::reply_desktop_query(&mut socket, &reply)
                                    .await
                            {
                                tracing::debug!(%error, "application query response failed");
                                break;
                            }
                        }
                        Ok(Ok(pab_executor::local_ipc::LocalEvent::DesktopQuery(
                            id,
                            query,
                            context,
                        ))) => {
                            let mut reply = if !cfg!(any(windows, target_os = "macos"))
                                || session_helper::desktop_is_active(Some("Default"))
                            {
                                desktop_session.query_guarded_context(id, &query, context, || {
                                    if !cfg!(any(windows, target_os = "macos"))
                                        || session_helper::desktop_is_active(Some("Default"))
                                    {
                                        Ok(())
                                    } else {
                                        Err("interactive desktop changed".into())
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
                            if cfg!(any(windows, target_os = "macos"))
                                && !session_helper::desktop_is_active(Some("Default"))
                            {
                                reply.state = "unconfirmed".into();
                                reply.error =
                                    Some("interactive desktop changed during operation".into());
                            }
                            if let Err(error) =
                                pab_executor::local_ipc::reply_desktop_query(&mut socket, &reply)
                                    .await
                            {
                                tracing::debug!(%error,"desktop query response failed");
                                break;
                            }
                        }
                        Ok(Ok(pab_executor::local_ipc::LocalEvent::ListWindows)) => {
                            let entries = window_session::list_windows();
                            if let Err(error) =
                                pab_executor::local_ipc::reply_window_list(&mut socket, &entries)
                                    .await
                            {
                                tracing::debug!(%error, "window helper response failed");
                                break;
                            }
                        }
                        Ok(Ok(pab_executor::local_ipc::LocalEvent::CaptureScreenshot)) => {
                            let result = screenshot_session::capture_png();
                            let reply = match result {
                                Ok(bytes) => {
                                    pab_executor::local_ipc::reply_screenshot(&mut socket, &bytes)
                                        .await
                                }
                                Err(message) => {
                                    pab_executor::local_ipc::reply_screenshot_error(
                                        &mut socket,
                                        &message,
                                    )
                                    .await
                                }
                            };
                            if let Err(error) = reply {
                                tracing::debug!(%error, "screenshot helper response failed");
                                break;
                            }
                        }
                        Ok(Ok(pab_executor::local_ipc::LocalEvent::CaptureScreenshotV2(
                            options,
                        ))) => {
                            let result = if !cfg!(any(windows, target_os = "macos"))
                                || session_helper::desktop_is_active(Some("Default"))
                            {
                                if options.window_ref.is_some() {
                                    desktop_session.capture_window(&options)
                                } else {
                                    screenshot_session::capture(&options)
                                }
                            } else {
                                Err("interactive desktop changed".into())
                            };
                            let reply = match result {
                                Ok(image)
                                    if !cfg!(any(windows, target_os = "macos"))
                                        || session_helper::desktop_is_active(Some("Default")) =>
                                {
                                    pab_executor::local_ipc::reply_screenshot_v2(
                                        &mut socket,
                                        &image,
                                    )
                                    .await
                                }
                                Ok(_) => {
                                    pab_executor::local_ipc::reply_screenshot_error(
                                        &mut socket,
                                        "interactive desktop changed during capture",
                                    )
                                    .await
                                }
                                Err(message) => {
                                    pab_executor::local_ipc::reply_screenshot_error(
                                        &mut socket,
                                        &message,
                                    )
                                    .await
                                }
                            };
                            if let Err(error) = reply {
                                tracing::debug!(%error,"screenshot helper response failed");
                                break;
                            }
                        }
                        Ok(Ok(pab_executor::local_ipc::LocalEvent::DesktopInput(event))) => {
                            let result = if cfg!(any(windows, target_os = "macos"))
                                && !session_helper::desktop_is_active(Some("Default"))
                            {
                                Err("interactive desktop changed".to_owned())
                            } else {
                                desktop_input::apply(event)
                            };
                            if let Err(error) = pab_executor::local_ipc::reply_desktop_input(
                                &mut socket,
                                result.as_ref().map(|_| ()).map_err(String::as_str),
                            )
                            .await
                            {
                                tracing::debug!(%error, "desktop input response failed");
                                break;
                            }
                        }
                        Ok(Err(error)) => {
                            tracing::debug!(%error, "local WebSocket disconnected");
                            break;
                        }
                        // This one-second wake-up checks whether Windows changed
                        // desktops. Status updates arrive every three seconds;
                        // an idle tick is not a disconnected local service.
                        Err(_) => {
                            browser.expire_idle().await;
                            continue;
                        }
                    }
                }
            }
            Err(error) => tracing::debug!(%error, "local WebSocket unavailable"),
        }
        *status.0.write().await = None;
        let _ = handle.emit("local-device-offline", ());
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

#[cfg(windows)]
pub fn run_session_supervisor() -> Result<(), String> {
    windows_session_supervisor::run()
}

#[cfg(windows)]
pub fn run_media_supervisor() -> Result<(), String> {
    windows_session_supervisor::run_media_only()
}
#[cfg(target_os = "linux")]
pub fn run_media_supervisor() -> Result<(), String> {
    linux_media_supervisor::run()
}

pub fn run_session_helper() -> Result<(), String> {
    session_helper::run()
}

#[cfg(target_os = "linux")]
pub fn linux_user_data_path() -> Result<std::path::PathBuf, String> {
    server_settings::apply_saved_at_start()?;
    DataPaths::for_scope(DataScope::User).map(|p| p.root().to_owned()).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Err(error) = server_settings::apply_saved_at_start() {
        eprintln!("Could not apply saved operator server settings: {error}");
        return;
    }
    let paths = DataPaths::for_scope(DataScope::User).expect("could not find user data directory");
    #[cfg(windows)]
    let gui_instance = match windows_gui::GuiInstance::acquire(paths.root()) {
        Ok(Some(instance)) => instance,
        Ok(None) => return,
        Err(error) => {
            eprintln!("Could not open desktop window: {error}");
            return;
        }
    };
    #[cfg(target_os = "macos")]
    let gui_instance = match macos_launch::GuiInstance::acquire(paths.root()) {
        Ok(Some(instance)) => instance,
        Ok(None) => return,
        Err(error) => {
            eprintln!("Could not open desktop window: {error}");
            return;
        }
    };
    pab_logging::init("desktop", paths.root()).expect("could not initialize desktop log file");
    #[cfg(target_os = "linux")]
    let gui_instance = match linux_launch::GuiInstance::acquire(paths.root()) {
        Ok(Some(instance)) => instance,
        Ok(None) => return,
        Err(error) => {
            eprintln!("Could not open desktop window: {error}");
            return;
        }
    };
    #[cfg(target_os = "linux")]
    {
        // The GUI entry point runs on the main thread, before Tauri creates
        // any GTK windows. Media/session helper entry points do not use this.
        let installed = unsafe { pab_desktop_media::initialize_linux_accessibility() };
        tracing::debug!(installed, "checked GTK button accessibility compatibility");
    }
    let context = tauri::generate_context!();
    #[cfg(target_os = "linux")]
    let context = {
        let mut context = context;
        if std::env::args_os().any(|a| a == "--background") {
            for window in &mut context.config_mut().app.windows { window.visible = false; }
        }
        context
    };
    #[cfg(all(windows, debug_assertions))]
    let context = {
        let mut context = context;
        if let Some(arguments) = operator::desktop_terminal::acceptance_browser_arguments() {
            for window in &mut context.config_mut().app.windows {
                window.additional_browser_args = Some(arguments.clone());
            }
        }
        context
    };
    tauri::Builder::default()
        .manage(LocalStatus::default())
        .manage(mcp_reporting::McpReportingState::default())
        .manage(desktop_assistance::AssistanceState::default())
        .manage(operator::OperatorState::new())
        .manage(operator::desktop_terminal::DesktopTerminalState::default())
        .manage(operator::remote_desktop::RemoteDesktopState::default())
        .manage(github_login::GithubLoginState::default())
        .manage(update::UpdateState::default())
        .on_window_event(tray::on_window_event)
        .setup(move |app| {
            let device_accounts = device_account::DeviceAccountService::default();
            app.manage(device_accounts.clone());
            tauri::async_runtime::spawn(device_accounts.run());
            tauri::async_runtime::spawn(device_account::run_catalog());
            tray::setup(app)?;
            #[cfg(target_os = "linux")]
            {
                gui_instance.listen(app.handle().clone())?;
                app.manage(gui_instance);
                match session_helper::linux_media::start() {
                    Ok(media) => { app.manage(media); }
                    Err(error) => tracing::warn!(%error, "Linux media helper is unavailable"),
                }
            }
            tauri::async_runtime::spawn(show_pending_browser_handoff(app.handle().clone()));
            let update_state = app.state::<update::UpdateState>().inner().clone();
            update::start_background_checks(app.handle().clone(), update_state);
            #[cfg(windows)]
            {
                gui_instance.listen(app.handle().clone())?;
                app.manage(gui_instance);
            }
            #[cfg(target_os = "macos")]
            {
                gui_instance.listen(app.handle().clone())?;
                app.manage(gui_instance);
            }
            let reporting = app
                .state::<mcp_reporting::McpReportingState>()
                .inner()
                .clone();
            tauri::async_runtime::block_on(mcp_reporting::start(app.handle().clone(), reporting));
            desktop_assistance::start(
                app.handle().clone(),
                app.state::<desktop_assistance::AssistanceState>()
                    .inner()
                    .clone(),
            );
            let handle = app.handle().clone();
            let status = app.state::<LocalStatus>().inner().clone();
            tauri::async_runtime::spawn(watch_local_service(handle, status));
            Ok(())
        })
        .invoke_handler(|invoke| {
            if invoke.message.webview().label().starts_with("desktop-viewer-")
                && !invoke.message.command().starts_with("desktop_viewer_") {
                invoke.resolver.reject("command is unavailable in a remote desktop window");
                return true;
            }
            // A terminal WebView cannot call stable operator/account/task APIs.
            // Every allowed terminal command additionally checks its native label.
            if invoke
                .message
                .webview()
                .label()
                .starts_with("desktop-terminal-")
                && !invoke.message.command().starts_with("desktop_terminal_")
            {
                invoke
                    .resolver
                    .reject("command is unavailable in a remote terminal window");
                return true;
            }
            let handler: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![
                operator::remote_desktop::desktop_viewer_window,
                operator::remote_desktop::desktop_viewer_start,
                operator::remote_desktop::desktop_viewer_control,
                operator::remote_desktop::desktop_viewer_overlay,
                operator::remote_desktop::desktop_viewer_fullscreen,
                desktop_assistance::desktop_assistance_pending,
                desktop_assistance::desktop_assistance_displayed,
                desktop_assistance::desktop_assistance_open,
                desktop_assistance::desktop_assistance_cancel,
                desktop_assistance::desktop_viewer_assistance,
                desktop_assistance::desktop_viewer_assistance_complete,
                operator::desktop_terminal::desktop_terminal_window,
                operator::desktop_terminal::desktop_terminal_info,
                operator::desktop_terminal::desktop_terminal_accounts,
                operator::desktop_terminal::desktop_terminal_selection,
                operator::desktop_terminal::desktop_terminal_start,
                operator::desktop_terminal::desktop_terminal_watch,
                operator::desktop_terminal::desktop_terminal_input,
                operator::desktop_terminal::desktop_terminal_resize,
                operator::desktop_terminal::desktop_terminal_consume,
                operator::desktop_terminal::desktop_terminal_end,
                operator::desktop_terminal::desktop_terminal_destroy,
                tray::set_tray_language,
                update::check_for_update,
                update::update_installation_mode,
                update::update_status,
                update::download_update,
                update::install_update,
                macos_permissions::macos_permissions,
                macos_permissions::macos_permission_targets,
                macos_permissions::macos_file_access_check,
                macos_permissions::reveal_macos_permission_target,
                macos_permissions::restart_macos_executor_for_permissions,
                macos_permissions::request_macos_permission,
                macos_permissions::open_macos_permission_settings,
                macos_permissions::restart_macos_permission_processes,
                device_status,
                rotate_device_password,
                browser_setup::browser_runtime_info,
                browser_setup::browser_profile_status,
                browser_setup::browser_profile_clear,
                browser_setup::browser_handoff_status,
                browser_setup::browser_handoff_complete,
                browser_setup::browser_handoff_cancel,
                device_account::device_account_status,
                device_account::device_account_associate,
                device_account::account_catalog_status,
                device_account::account_catalog_action,
                mcp_reporting::mcp_reporting_status,
                mcp_tool_settings::get_mcp_tool_settings,
                mcp_tool_settings::save_mcp_tool_settings,
                server_settings::get_operator_server_settings,
                server_settings::validate_operator_server,
                server_settings::save_operator_server_settings,
                server_settings::restart_desktop,
                agent_integrations::agent_integration_status,
                agent_integrations::set_agent_integration,
                operator::operator_connect,
                operator::operator_connect_saved,
                operator::operator_disconnect_device,
                operator::operator_forget_device,
                operator::operator_saved_device_presence,
                operator::operator_connection_paths,
                operator::operator_presence,
                operator::history::operator_bootstrap,
                operator::history::operator_history_page,
                operator::history::operator_device_history_page,
                operator::history::operator_operations,
                operator::history::operator_rename_device,
                operator::operator_run_command,
                operator::execution::operator_execution_query,
                operator::execution::operator_execution_query_result,
                operator::transfer::operator_start_transfer,
                operator::transfer::operator_cancel_transfer,
                operator::transfer::operator_transfer_result,
                operator::directory::operator_list_directory,
                operator::windows::operator_list_windows,
                operator::windows::operator_desktop_input,
                operator::screenshot::operator_capture_screenshot,
                operator::screenshot::operator_preview_desktop,
                operator::screenshot::operator_screenshot_from_history,
                operator::terminal::operator_open_terminal,
                operator::terminal::operator_terminal_input,
                operator::terminal::operator_terminal_read,
                operator::terminal::operator_terminal_resize,
                operator::terminal::operator_terminal_close,
                operator::terminal::operator_terminal_history,
                operator::operator_task,
                operator::operator_current_traffic_scope,
                operator::operator_login_account,
                github_login::github_enabled,
                github_login::github_login,
                github_login::github_cancel,
                github_login::github_status,
                github_login::github_unlink,
                operator::operator_register_account,
                operator::operator_use_guest_scope,
            ];
            handler(invoke)
        })
        .build(context)
        .expect("could not start Pixels Agent Bridge desktop application")
        .run(|handle, event| match event {
            tauri::RunEvent::ExitRequested { code, api, .. }
                if tray::prevent_exit(handle, code) =>
            {
                api.prevent_exit()
            }
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => tray::show_window(handle),
            tauri::RunEvent::Exit => {
                handle.state::<mcp_reporting::McpReportingState>().stop();
                handle.state::<desktop_assistance::AssistanceState>().stop();
            }
            _ => {}
        });
}
