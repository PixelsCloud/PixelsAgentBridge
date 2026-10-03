use pab_agent_core::{DataPaths, DataScope};
use pab_executor::{DeviceStatus, read_local_device_status};
use pab_protocol::ClaimId;
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tokio::sync::RwLock;

mod agent_integrations;
mod desktop_input;
mod mcp_reporting;
mod mcp_tool_settings;
mod operator;
mod screenshot_session;
mod server_settings;
mod session_helper;
mod window_session;
#[cfg(windows)]
mod windows_session_supervisor;

#[derive(Clone, Default)]
struct LocalStatus(Arc<RwLock<Option<DeviceStatus>>>);

#[tauri::command]
async fn device_status(state: tauri::State<'_, LocalStatus>) -> Result<DeviceStatus, String> {
    if let Some(status) = state.0.read().await.clone() {
        return Ok(status);
    }
    read_local_device_status()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn approve_claim(claim_id: String) -> Result<(), String> {
    let claim_id = claim_id
        .parse::<ClaimId>()
        .map_err(|_| "申请 ID 格式不正确".to_owned())?;
    let token_exists = pab_executor::local_ipc::user_token_path()
        .ok()
        .is_some_and(|path| path.is_file());
    if token_exists {
        pab_executor::local_ipc::approve_local_claim(claim_id)
            .await
            .map_err(|error| error.to_string())
    } else {
        pab_executor::approve_claim(claim_id)
            .await
            .map_err(|error| {
                tracing::warn!(%error, "device claim approval failed");
                error.to_string()
            })
    }
}

#[tauri::command]
async fn pending_device_claims() -> Result<Vec<pab_protocol::DeviceClaimEntry>,String> {
    let token_exists=pab_executor::local_ipc::user_token_path().ok().is_some_and(|path|path.is_file());
    if token_exists {pab_executor::local_ipc::local_claim_request(None).await.map_err(|e|e.to_string())}
    else {pab_executor::list_device_claims().await.map_err(|e|e.to_string())}
}

#[tauri::command]
async fn reject_device_claim(claim_id:String)->Result<(),String>{
    let id=claim_id.parse::<ClaimId>().map_err(|_|"invalid claim id".to_owned())?;
    let token_exists=pab_executor::local_ipc::user_token_path().ok().is_some_and(|path|path.is_file());
    if token_exists {pab_executor::local_ipc::local_claim_request(Some(id)).await.map(|_|()).map_err(|e|e.to_string())}
    else {pab_executor::reject_claim(id).await.map_err(|e|e.to_string())}
}

async fn watch_local_service(handle: tauri::AppHandle, status: LocalStatus) {
    loop {
        match pab_executor::local_ipc::connect_local().await {
            Ok(mut socket) => {
                #[cfg(windows)]
                if !session_helper::desktop_is_active(Some("Default")) {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
                #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
                if let Err(error) =
                    pab_executor::local_ipc::register_window_helper(&mut socket).await
                {
                    tracing::debug!(%error, "window helper registration failed");
                }
                let mut desktop_session = pab_desktop_control::DesktopSession::new();
                loop {
                    #[cfg(windows)]
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
                            *status.0.write().await = Some(next.clone());
                            let _ = handle.emit("local-device-status", next);
                        }
                        Ok(Ok(pab_executor::local_ipc::LocalEvent::StatusUnavailable(message))) => {
                            tracing::debug!(%message, "local device status unavailable");
                            *status.0.write().await = None;
                            let _ = handle.emit("local-device-offline", ());
                        }
                        Ok(Ok(pab_executor::local_ipc::LocalEvent::DesktopQuery(id, query))) => {
                            let mut reply = if !cfg!(windows)
                                || session_helper::desktop_is_active(Some("Default"))
                            {
                                desktop_session.query(id, &query)
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
                            if cfg!(windows) && !session_helper::desktop_is_active(Some("Default"))
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
                            let result = if !cfg!(windows)
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
                                    if !cfg!(windows)
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
                            let result = if cfg!(windows)
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
                        Err(_) => break,
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

pub fn run_session_helper() -> Result<(), String> {
    session_helper::run()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Err(error) = server_settings::apply_saved_at_start() {
        eprintln!("Could not apply saved operator server settings: {error}");
    }
    let paths = DataPaths::for_scope(DataScope::User).expect("could not find user data directory");
    pab_logging::init("desktop", paths.root()).expect("could not initialize desktop log file");
    tauri::Builder::default()
        .manage(LocalStatus::default())
        .manage(mcp_reporting::McpReportingState::default())
        .manage(operator::OperatorState::new())
        .setup(|app| {
            let reporting = app
                .state::<mcp_reporting::McpReportingState>()
                .inner()
                .clone();
            tauri::async_runtime::block_on(mcp_reporting::start(app.handle().clone(), reporting));
            let handle = app.handle().clone();
            let status = app.state::<LocalStatus>().inner().clone();
            tauri::async_runtime::spawn(watch_local_service(handle, status));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            device_status,
            mcp_reporting::mcp_reporting_status,
            approve_claim,
            pending_device_claims,
            reject_device_claim,
            mcp_tool_settings::get_mcp_tool_settings,
            mcp_tool_settings::save_mcp_tool_settings,
            server_settings::get_operator_server_settings,
            server_settings::save_operator_server_settings,
            server_settings::restart_desktop,
            agent_integrations::codex_integration_status,
            agent_integrations::set_codex_integration,
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
            operator::operator_start_transfer,
            operator::operator_cancel_transfer,
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
            operator::operator_claim,
            operator::operator_current_traffic_scope,
            operator::operator_login_account,
            operator::operator_use_guest_scope,
        ])
        .build(tauri::generate_context!())
        .expect("could not start Pixels Agent Bridge desktop application")
        .run(|handle, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                handle.state::<mcp_reporting::McpReportingState>().stop();
            }
        });
}
