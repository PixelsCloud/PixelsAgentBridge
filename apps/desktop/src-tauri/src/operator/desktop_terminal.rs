use super::*;
use pab_bridge::DesktopTerminalSession;
use pab_protocol::{
    DesktopTerminalEvent, DesktopTerminalOpened, ExecutionContextEntry, ExecutionMode,
    ExecutionSelection, OsSessionInfo, SystemQuery, SystemQueryData, SystemQueryReply,
};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{
    Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, ipc::Channel,
};

#[derive(serde::Serialize)]
pub struct StartedView {
    #[serde(flatten)]
    opened: DesktopTerminalOpened,
    connection_path: &'static str,
    os_family: pab_protocol::OsFamily,
}

#[derive(Clone, Copy, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalLaunchMode {
    Console,
    System,
}

#[derive(serde::Serialize)]
pub struct TerminalWindowInfo {
    code: String,
    launch_mode: Option<TerminalLaunchMode>,
}

#[derive(Default)]
pub struct DesktopTerminalState {
    windows: Mutex<HashMap<String, Arc<WindowOwner>>>,
}
/// Opt-in instrumentation for isolated native acceptance. This function and
/// its callers are absent from every release build; defaults remain unchanged.
#[cfg(all(windows, debug_assertions))]
pub(crate) fn acceptance_browser_arguments() -> Option<String> {
    if std::env::var("PAB_DESKTOP_TERMINAL_ACCEPTANCE").as_deref() != Ok("1") {
        return None;
    }
    // Separate isolated Desktop processes need separate loopback listeners.
    // This invocation option is only available in opt-in debug builds.
    let port = match std::env::args()
        .find_map(|arg| arg.strip_prefix("--acceptance-cdp-port=").map(str::to_owned))
    {
        Some(value) => value.parse::<u16>().ok().filter(|port| *port >= 1024)?,
        None => 39100,
    };
    Some(format!("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --remote-debugging-port={port} --remote-debugging-address=127.0.0.1"))
}
struct WindowOwner {
    code: DeviceCode,
    launch_mode: Option<TerminalLaunchMode>,
    account: String,
    runtime: Arc<BridgeRuntime>,
    opening: AtomicBool,
    alive: AtomicBool,
    window_closed: tokio::sync::watch::Sender<bool>,
    session: Mutex<Option<Arc<DesktopTerminalSession>>>,
    output: Mutex<Option<tokio::task::JoinHandle<()>>>,
    ready: Mutex<Option<(RequestId, Arc<tokio::sync::Semaphore>)>>,
    inputs: tokio::sync::Semaphore,
}
fn current_account() -> Result<String, String> {
    let snapshot = pab_agent_core::account::AccountStore::from_env()
        .and_then(|s| s.read())
        .map_err(|e| e.to_string())?;
    snapshot
        .user
        .map(|u| u.id.to_string())
        .ok_or_else(|| "login required to control a remote computer".into())
}
fn main_window(window: &WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        Err("only the main window can create remote terminals".into())
    } else {
        Ok(())
    }
}
impl DesktopTerminalState {
    async fn owner(&self, window: &WebviewWindow) -> Result<Arc<WindowOwner>, String> {
        let owner = self
            .windows
            .lock()
            .await
            .get(window.label())
            .cloned()
            .ok_or("this window does not own a terminal")?;
        if owner.account != current_account()? {
            return Err(
                "terminal belongs to a different signed-in account; close and reopen it".into(),
            );
        }
        Ok(owner)
    }
    pub(crate) async fn close_window(&self, label: &str) -> Result<(), String> {
        let owner = self.windows.lock().await.get(label).cloned();
        if let Some(owner) = owner {
            owner.alive.store(false, Ordering::Release);
            owner.window_closed.send_replace(true);
            if let Some(session) = owner.session.lock().await.clone() {
                session.close().await.map_err(|e| e.to_string())?;
            }
            if let Some(pump) = owner.output.lock().await.take() {
                pump.abort();
            }
            self.windows.lock().await.remove(label);
        }
        Ok(())
    }
}
#[tauri::command]
pub async fn desktop_terminal_window(
    app: tauri::AppHandle,
    window: WebviewWindow,
    state: State<'_, OperatorState>,
    terminals: State<'_, DesktopTerminalState>,
    code: String,
    mode: Option<TerminalLaunchMode>,
) -> Result<(), String> {
    main_window(&window)?;
    let account = current_account()?;
    let code = parse_code(&code)?;
    let runtime = state.runtime().await?;
    // Only an already-added device may be used. Existing saved-credential
    // connection preparation is performed by the main device workflow.
    let saved = runtime
        .remembered_devices()
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|d| d.code == code)
        .ok_or("device is not in the added device list")?;
    let label = format!("desktop-terminal-{}", RequestId::new());
    let mut owners = terminals.windows.lock().await;
    if owners.len() >= 8 {
        return Err("at most eight remote terminal windows can be opened".into());
    }
    let (window_closed, _) = tokio::sync::watch::channel(false);
    let owner = Arc::new(WindowOwner {
        code,
        launch_mode: mode,
        account,
        runtime,
        opening: AtomicBool::new(false),
        alive: AtomicBool::new(true),
        window_closed,
        session: Mutex::new(None),
        output: Mutex::new(None),
        ready: Mutex::new(None),
        inputs: tokio::sync::Semaphore::new(16),
    });
    owners.insert(label.clone(), owner.clone());
    drop(owners);
    let title = if saved.alias.is_empty() {
        format!("Pixels · {code} · Remote terminal")
    } else {
        format!("Pixels · {} · {code}", saved.alias)
    };
    let builder = WebviewWindowBuilder::new(
        &app,
        &label,
        WebviewUrl::App(format!("index.html?desktopTerminal={label}").into()),
    )
    .title(title)
    .inner_size(1100.0, 720.0)
    .min_inner_size(640.0, 400.0)
    .resizable(true)
    .maximizable(true)
    .decorations(false);
    #[cfg(all(windows, debug_assertions))]
    let builder = if let Some(arguments) = acceptance_browser_arguments() {
        builder.additional_browser_args(&arguments).initialization_script("window.__PIXELS_TERMINAL_ACCEPTANCE__ = {onFrame: null, paused: false, pending: [], pendingBytes: 0, maxPendingBytes: 0};")
    } else {
        builder
    };
    let built = builder.build();
    match built {
        Ok(created) => {
            let mut changes = pab_agent_core::account::account_changes();
            let account_app = app.clone();
            let account_label = label.clone();
            let account_owner = Arc::downgrade(&owner);
            let mut window_closed = owner.window_closed.subscribe();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::select! {
                        changed = changes.changed() => if changed.is_err() { break; },
                        _ = window_closed.wait_for(|v| *v) => break,
                    }
                    let Some(owner) = account_owner.upgrade() else {
                        break;
                    };
                    if !owner.alive.load(Ordering::Acquire) {
                        break;
                    }
                    if current_account().ok().as_ref() != Some(&owner.account) {
                        let _ = account_app.emit_to(&account_label, "desktop-terminal-account-ended", "signed-in account changed; reopen a terminal under the current account");
                        let _ = account_app
                            .state::<DesktopTerminalState>()
                            .close_window(&account_label)
                            .await;
                        break;
                    }
                }
            });
            let app = app.clone();
            let destroyed_label = label.clone();
            created.on_window_event(move |event| {
                if matches!(event, tauri::WindowEvent::Destroyed) {
                    let app = app.clone();
                    let label = destroyed_label.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = app
                            .state::<DesktopTerminalState>()
                            .close_window(&label)
                            .await
                        {
                            tracing::warn!(error = %e, "remote terminal cleanup unconfirmed");
                        }
                    });
                }
            });
            let _ = created.set_focus();
            Ok(())
        }
        Err(e) => {
            terminals.windows.lock().await.remove(&label);
            Err(e.to_string())
        }
    }
}
#[tauri::command]
pub async fn desktop_terminal_info(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
) -> Result<TerminalWindowInfo, String> {
    let owner = state.owner(&window).await?;
    Ok(TerminalWindowInfo {
        code: owner.code.to_string(),
        launch_mode: owner.launch_mode,
    })
}
#[tauri::command]
pub async fn desktop_terminal_accounts(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
) -> Result<SystemQueryReply, String> {
    let owner = state.owner(&window).await?;
    terminal_query(
        &owner,
        SystemQuery::ExecutionContexts {
            user: None,
            include_system: false,
            limit: 1000,
        },
    )
    .await
}

async fn terminal_query(
    owner: &WindowOwner,
    query: SystemQuery,
) -> Result<SystemQueryReply, String> {
    let device = owner
        .runtime
        .resolve_device_code(owner.code)
        .await
        .map_err(|e| e.to_string())?;
    let id = RequestId::new();
    let mut reply = owner
        .runtime
        .system_query(device, id, query)
        .await
        .map_err(|e| e.to_string())?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    while matches!(reply.state.as_str(), "queued" | "running")
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        reply = owner
            .runtime
            .get_system_query(device, id)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(reply)
}

fn reply_data(reply: SystemQueryReply) -> Result<(SystemQueryData, bool), String> {
    if reply.state != "completed" {
        return Err(reply.error.unwrap_or_else(|| {
            "remote account/session query did not complete; retry opening the terminal".into()
        }));
    }
    Ok((
        reply
            .data
            .ok_or("remote account/session query returned no data")?,
        reply.truncated,
    ))
}

fn console_selection(
    entries: &[ExecutionContextEntry],
    backend: &str,
    sessions: &[OsSessionInfo],
) -> Result<ExecutionSelection, String> {
    let consoles: Vec<_> = sessions
        .iter()
        .filter(|s| {
            s.active == Some(true)
                && s.remote == Some(false)
                && s.user_name.is_some()
                && (s.session_type.as_deref() == Some("console")
                    || s.terminal.as_deref() == Some("console")
                    || s.seat.as_deref().is_some_and(|seat| !seat.is_empty()))
        })
        .collect();
    let [console] = consoles.as_slice() else {
        return Err(if consoles.is_empty() { "no active local console login was found; log in on the remote computer, or choose Remote terminal (System)" } else { "multiple active local console sessions were found; a unique console account could not be selected" }.into());
    };
    let matching: Vec<_> = entries
        .iter()
        .filter(|entry| {
            if entry.mode != ExecutionMode::User || entry.unavailable_reason.is_some() {
                return false;
            }
            let Some(identity) = &entry.identity else {
                return false;
            };
            if backend == "windows_wts" {
                let name = console.user_name.as_deref().unwrap_or_default();
                let name = console
                    .domain
                    .as_deref()
                    .filter(|d| !d.is_empty())
                    .map_or_else(|| name.to_string(), |domain| format!("{domain}\\{name}"));
                identity.session_id.as_deref() == Some(console.id.as_str())
                    && identity.account_name.eq_ignore_ascii_case(&name)
            } else {
                console
                    .user_id
                    .is_some_and(|uid| identity.account_id == format!("uid:{uid}"))
            }
        })
        .filter_map(|entry| match entry.selection {
            Some(selection @ ExecutionSelection::User { .. }) => Some(selection),
            _ => None,
        })
        .collect();
    let [selection] = matching.as_slice() else {
        return Err("the current console account is unavailable or ambiguous; retry after the remote login/session is ready".into());
    };
    Ok(*selection)
}

fn system_selection(entries: &[ExecutionContextEntry]) -> Result<ExecutionSelection, String> {
    let service = entries
        .iter()
        .find(|entry| entry.mode == ExecutionMode::Service)
        .ok_or("remote service account was not reported")?;
    let identity = service
        .identity
        .as_ref()
        .ok_or("remote service account is unavailable")?;
    if !matches!(identity.account_id.as_str(), "S-1-5-18" | "uid:0") {
        return Err(
            "remote service is not running as System/root; cannot open a System terminal".into(),
        );
    }
    match service.selection {
        Some(selection @ ExecutionSelection::Service { .. })
            if service.unavailable_reason.is_none() =>
        {
            Ok(selection)
        }
        _ => Err("remote System execution context is unavailable".into()),
    }
}

#[tauri::command]
pub async fn desktop_terminal_selection(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
) -> Result<ExecutionSelection, String> {
    let owner = state.owner(&window).await?;
    let mode = owner
        .launch_mode
        .ok_or("this terminal was opened without an automatic account mode")?;
    let (contexts, truncated) = reply_data(
        terminal_query(
            &owner,
            SystemQuery::ExecutionContexts {
                user: None,
                include_system: false,
                limit: 1000,
            },
        )
        .await?,
    )?;
    let SystemQueryData::ExecutionContexts { entries, backend } = contexts else {
        return Err("unexpected remote account reply".into());
    };
    // Android has one application UID, not a desktop console login or a root
    // service. Only the normal console launch maps to that advertised identity.
    if backend == "android_app_uid" {
        if matches!(mode, TerminalLaunchMode::System) {
            return Err("Android does not provide a System/root terminal".into());
        }
        return android_app_selection(&entries);
    }
    if matches!(mode, TerminalLaunchMode::System) {
        return system_selection(&entries);
    }
    if truncated {
        return Err(
            "remote account inventory is incomplete; cannot identify the console account reliably"
                .into(),
        );
    }
    let (sessions, truncated) = reply_data(
        terminal_query(
            &owner,
            SystemQuery::Sessions {
                user: None,
                state: None,
                limit: 1000,
            },
        )
        .await?,
    )?;
    if truncated {
        return Err(
            "remote session inventory is incomplete; cannot identify the console account reliably"
                .into(),
        );
    }
    let SystemQueryData::Sessions {
        backend,
        entries: sessions,
    } = sessions
    else {
        return Err("unexpected remote session reply".into());
    };
    console_selection(&entries, &backend, &sessions)
}
pub(super) fn android_app_selection(entries:&[ExecutionContextEntry])->Result<ExecutionSelection,String> {
    let entries:Vec<_>=entries.iter().filter(|e|e.mode==ExecutionMode::Service && e.unavailable_reason.is_none()
        && e.identity.as_ref().is_some_and(|i|i.mode==ExecutionMode::Service && i.account_id.starts_with("uid:") && i.account_id!="uid:0" && i.home=="/workspace")
        && matches!(e.selection,Some(ExecutionSelection::Service {..}))).collect();
    if entries.len()!=1 {return Err("Android application execution context is unavailable".into());}
    Ok(entries[0].selection.expect("validated service selection"))
}
#[tauri::command]
pub async fn desktop_terminal_start(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
    execution: ExecutionSelection,
    cols: u16,
    rows: u16,
    events: Channel<tauri::ipc::Response>,
) -> Result<StartedView, String> {
    let owner = state.owner(&window).await?;
    if owner.opening.swap(true, Ordering::AcqRel) {
        return Err("a terminal start is already in progress".into());
    }
    let result = async {
        if owner.session.lock().await.is_some() {
            return Err("close the existing Shell before reopening".into());
        }
        let device = owner
            .runtime
            .resolve_device_code(owner.code)
            .await
            .map_err(|e| e.to_string())?;
        let session = owner
            .runtime
            .open_desktop_terminal(device, execution, cols, rows)
            .await
            .map_err(|e| e.to_string())?;
        if !owner.alive.load(Ordering::Acquire) || owner.account != current_account()? {
            let _ = session.close().await;
            return Err("window or signed-in account changed during terminal startup".into());
        }
        let opened = session.opened.clone();
        let os_family = session.os_family;
        let connection_path = match session.connection_path() {
            Some(pab_bridge::ConnectionPath::Direct) => "P2P",
            Some(pab_bridge::ConnectionPath::Relay) => "Relay",
            _ => "Unknown",
        };
        *owner.session.lock().await = Some(session.clone());
        let ready = Arc::new(tokio::sync::Semaphore::new(0));
        *owner.ready.lock().await = Some((opened.session_id, ready.clone()));
        let pump = tokio::spawn(async move {
            // The frontend must know the exact session ID before xterm can send
            // startup cursor responses or consumption ACKs for this Shell.
            let Ok(_permit) = ready.acquire().await else {
                return;
            };
            loop {
                match session.next_event().await {
                    Ok(event) => {
                        let ended = matches!(event, DesktopTerminalEvent::Ended { .. });
                        if send_event(&events, event).is_err() {
                            let _ = session.close().await;
                            break;
                        }
                        if ended {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = send_event(
                            &events,
                            DesktopTerminalEvent::Ended {
                                offset: 0,
                                exit_code: None,
                                output_bytes: None,
                                sha256: None,
                                reason: format!(
                                    "connection interrupted: {e}; input was not replayed"
                                ),
                            },
                        );
                        let _ = session.close().await;
                        break;
                    }
                }
            }
        });
        *owner.output.lock().await = Some(pump);
        Ok(StartedView {
            opened,
            connection_path,
            os_family,
        })
    }
    .await;
    owner.opening.store(false, Ordering::Release);
    result
}
fn send_event(
    events: &Channel<tauri::ipc::Response>,
    event: DesktopTerminalEvent,
) -> Result<(), String> {
    let mut frame = Vec::new();
    match event {
        DesktopTerminalEvent::Output { offset, bytes } => {
            frame.push(0);
            frame.extend_from_slice(&offset.to_be_bytes());
            frame.extend_from_slice(&bytes);
        }
        DesktopTerminalEvent::Ended {
            offset,
            exit_code,
            reason,
            output_bytes,
            sha256,
        } => {
            frame.push(1);
            frame.extend_from_slice(&offset.to_be_bytes());
            frame.extend_from_slice(
                &serde_json::to_vec(&serde_json::json!({"exit_code": exit_code, "reason": reason, "output_bytes": output_bytes, "sha256": sha256}))
                    .map_err(|e| e.to_string())?,
            );
        }
    }
    events
        .send(tauri::ipc::Response::new(frame))
        .map_err(|e| e.to_string())
}
async fn session(
    state: &DesktopTerminalState,
    window: &WebviewWindow,
    session_id: RequestId,
) -> Result<Arc<DesktopTerminalSession>, String> {
    let session = state
        .owner(window)
        .await?
        .session
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no active Shell in this window".to_owned())?;
    if session.opened.session_id != session_id {
        return Err("Shell changed; stale input or ACK was rejected".into());
    }
    Ok(session)
}
#[tauri::command]
pub async fn desktop_terminal_watch(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
    session_id: RequestId,
) -> Result<(), String> {
    session(&state, &window, session_id).await?;
    let owner = state.owner(&window).await?;
    let mut ready = owner.ready.lock().await;
    if !ready.as_ref().is_some_and(|(id, _)| *id == session_id) {
        return Err("terminal receiver is already attached or belongs to another Shell".into());
    }
    ready.take().unwrap().1.add_permits(1);
    Ok(())
}
#[tauri::command]
pub async fn desktop_terminal_connection_path(window:WebviewWindow,state:State<'_,DesktopTerminalState>,session_id:RequestId)->Result<Option<&'static str>,String> {
    let session=session(&state,&window,session_id).await?;
    Ok(match session.connection_path(){Some(pab_bridge::ConnectionPath::Direct)=>Some("P2P"),Some(pab_bridge::ConnectionPath::Relay)=>Some("Relay"),_=>None})
}
#[tauri::command]
pub async fn desktop_terminal_input(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
    session_id: RequestId,
    bytes: Vec<u8>,
) -> Result<(), String> {
    if bytes.is_empty() || bytes.len() > 4096 {
        return Err("input must contain 1..4096 bytes".into());
    }
    let owner = state.owner(&window).await?;
    let _slot = owner
        .inputs
        .try_acquire()
        .map_err(|_| "terminal input queue is full")?;
    session(&state, &window, session_id)
        .await?
        .input(bytes)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn desktop_terminal_resize(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
    session_id: RequestId,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    session(&state, &window, session_id)
        .await?
        .resize(cols, rows)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn desktop_terminal_consume(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
    session_id: RequestId,
    offset: u64,
) -> Result<(), String> {
    session(&state, &window, session_id)
        .await?
        .consume(offset)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn desktop_terminal_end(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
    session_id: Option<RequestId>,
) -> Result<(), String> {
    if !window.label().starts_with("desktop-terminal-") {
        return Err("this window does not own a terminal".into());
    }
    let Some(owner) = state.windows.lock().await.get(window.label()).cloned() else {
        return Ok(());
    };
    let terminal = owner.session.lock().await.clone();
    if let Some(terminal) = terminal {
        if session_id.is_some_and(|id| id != terminal.opened.session_id) {
            return Err("Shell changed; stale close was rejected".into());
        }
        terminal.close().await.map_err(|e| e.to_string())?;
    }
    if let Some(pump) = owner.output.lock().await.take() {
        pump.abort();
    }
    owner.session.lock().await.take();
    owner.ready.lock().await.take();
    Ok(())
}

#[tauri::command]
pub async fn desktop_terminal_destroy(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
) -> Result<(), String> {
    if !window.label().starts_with("desktop-terminal-") {
        return Err("only a remote terminal can close its own window".into());
    }
    // Resolve the caller's native window, never a renderer-supplied label.
    // Cleanup remains possible after logout has already removed its owner.
    state.close_window(window.label()).await?;
    window.destroy().map_err(|e| e.to_string())
}

#[cfg(test)]
mod launch_tests {
    use super::*;
    use pab_protocol::{ExecutionContextRef, ExecutionEnvironmentSource, ExecutionIdentity};

    fn account(session: &str, name: &str) -> ExecutionContextEntry {
        ExecutionContextEntry {
            mode: ExecutionMode::User,
            account_name: name.into(),
            account_id: Some("S-1-5-21-1001".into()),
            session_id: Some(session.into()),
            identity: Some(ExecutionIdentity {
                mode: ExecutionMode::User,
                account_id: "S-1-5-21-1001".into(),
                account_name: name.into(),
                home: "C:\\Users\\Alice".into(),
                primary_group: None,
                session_id: Some(session.into()),
                logon_id: Some("native-login".into()),
                environment_source: ExecutionEnvironmentSource::NativeAccount,
            }),
            selection: Some(ExecutionSelection::User {
                context_ref: ExecutionContextRef::new(),
            }),
            unavailable_reason: None,
        }
    }
    fn console(session: &str) -> OsSessionInfo {
        OsSessionInfo {
            id: session.into(),
            user_name: Some("Alice".into()),
            user_id: None,
            domain: Some("BOX".into()),
            state: "active".into(),
            active: Some(true),
            remote: Some(false),
            seat: None,
            terminal: Some("Console".into()),
            client_name: None,
            session_type: Some("console".into()),
            errors: vec![],
        }
    }
    #[test]
    fn current_console_is_selected_instead_of_another_active_or_disconnected_session() {
        let target = account("7", "BOX\\Alice");
        let others = account("42", "BOX\\Alice");
        let mut rdp = console("42");
        rdp.remote = Some(true);
        rdp.session_type = Some("rdp".into());
        let mut disconnected = console("9");
        disconnected.active = Some(false);
        disconnected.state = "disconnected".into();
        assert_eq!(
            console_selection(
                &[others, target.clone()],
                "windows_wts",
                &[rdp, disconnected, console("7")]
            )
            .unwrap(),
            target.selection.unwrap()
        );
    }
    #[test]
    fn no_console_and_multiple_consoles_never_fall_back_to_service_or_first_account() {
        let target = account("7", "BOX\\Alice");
        assert!(console_selection(&[target.clone()], "windows_wts", &[]).is_err());
        let mut rdp = console("7");
        rdp.remote = Some(true);
        rdp.session_type = Some("rdp".into());
        assert!(console_selection(&[target.clone()], "windows_wts", &[rdp]).is_err());
        assert!(
            console_selection(&[target], "windows_wts", &[console("7"), console("8")]).is_err()
        );
    }
    #[test]
    fn unavailable_stale_and_ambiguous_account_references_are_rejected() {
        let mut target = account("7", "BOX\\Alice");
        target.selection = None;
        assert!(console_selection(&[target], "windows_wts", &[console("7")]).is_err());
        assert!(
            console_selection(&[account("7", "BOX\\Bob")], "windows_wts", &[console("7")]).is_err()
        );
        let target = account("7", "BOX\\Alice");
        assert!(
            console_selection(&[target.clone(), target], "windows_wts", &[console("7")]).is_err()
        );
    }
    #[test]
    fn unix_console_is_matched_by_native_uid_not_account_order() {
        let mut session = console("utmpx:1");
        session.user_id = Some(501);
        session.domain = None;
        let mut target = account("user", "Alice");
        target.identity.as_mut().unwrap().account_id = "uid:501".into();
        let mut other = target.clone();
        other.identity.as_mut().unwrap().account_id = "uid:502".into();
        assert_eq!(
            console_selection(&[other, target.clone()], "macos_utmpx", &[session]).unwrap(),
            target.selection.unwrap()
        );
    }
    #[test]
    fn system_entry_must_be_native_system_or_root_and_selectable() {
        let mut service = account("0", "NT AUTHORITY\\SYSTEM");
        service.mode = ExecutionMode::Service;
        service.selection = Some(ExecutionSelection::Service {});
        let identity = service.identity.as_mut().unwrap();
        identity.mode = ExecutionMode::Service;
        identity.account_id = "S-1-5-18".into();
        assert_eq!(
            system_selection(&[service.clone()]).unwrap(),
            ExecutionSelection::Service {}
        );
        service.identity.as_mut().unwrap().account_id = "S-1-5-21-1001".into();
        assert!(system_selection(&[service.clone()]).is_err());
        service.identity.as_mut().unwrap().account_id = "uid:0".into();
        assert!(system_selection(&[service.clone()]).is_ok());
        service.unavailable_reason = Some("user_token_unavailable".into());
        assert!(system_selection(&[service.clone()]).is_err());
        service.unavailable_reason = None;
        service.selection = None;
        assert!(system_selection(&[service]).is_err());
        assert!(system_selection(&[]).is_err());
    }
    #[test]
    fn failed_and_pending_native_queries_are_not_treated_as_complete_inventories() {
        let query = SystemQuery::Sessions {
            user: None,
            state: None,
            limit: 1000,
        };
        let mut reply = SystemQueryReply::pending(RequestId::new(), &query);
        assert!(reply_data(reply.clone()).is_err());
        reply.state = "failed".into();
        reply.error = Some("native inventory failed".into());
        assert_eq!(
            reply_data(reply.clone()).unwrap_err(),
            "native inventory failed"
        );
        reply.state = "completed".into();
        reply.error = None;
        assert!(reply_data(reply.clone()).is_err());
        reply.data = Some(SystemQueryData::Sessions {
            backend: "windows_wts".into(),
            entries: vec![console("7")],
        });
        reply.truncated = true;
        assert!(reply_data(reply).unwrap().1);
        assert!(serde_json::from_str::<TerminalLaunchMode>("\"unknown\"").is_err());
    }
}
