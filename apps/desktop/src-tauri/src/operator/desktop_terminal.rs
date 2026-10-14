use super::*;
use pab_bridge::DesktopTerminalSession;
use pab_protocol::{
    DesktopTerminalEvent, DesktopTerminalOpened, ExecutionSelection, SystemQuery, SystemQueryReply,
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
}

#[derive(Default)]
pub struct DesktopTerminalState {
    windows: Mutex<HashMap<String, Arc<WindowOwner>>>,
}
/// Opt-in instrumentation for isolated native acceptance. This function and
/// its callers are absent from every release build; defaults remain unchanged.
#[cfg(all(windows, debug_assertions))]
pub(crate) fn acceptance_browser_arguments() -> Option<&'static str> {
    (std::env::var("PAB_DESKTOP_TERMINAL_ACCEPTANCE").as_deref() == Ok("1")).then_some("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --remote-debugging-port=39100 --remote-debugging-address=127.0.0.1")
}
struct WindowOwner {
    code: DeviceCode,
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
    .decorations(true);
    #[cfg(all(windows, debug_assertions))]
    let builder = if let Some(arguments) = acceptance_browser_arguments() {
        builder.additional_browser_args(arguments).initialization_script("window.__PIXELS_TERMINAL_ACCEPTANCE__ = {onFrame: null, paused: false, pending: [], pendingBytes: 0, maxPendingBytes: 0};")
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
) -> Result<String, String> {
    Ok(state.owner(&window).await?.code.to_string())
}
#[tauri::command]
pub async fn desktop_terminal_accounts(
    window: WebviewWindow,
    state: State<'_, DesktopTerminalState>,
) -> Result<SystemQueryReply, String> {
    let owner = state.owner(&window).await?;
    let device = owner
        .runtime
        .resolve_device_code(owner.code)
        .await
        .map_err(|e| e.to_string())?;
    let id = RequestId::new();
    let mut reply = owner
        .runtime
        .system_query(
            device,
            id,
            SystemQuery::ExecutionContexts {
                user: None,
                include_system: false,
                limit: 1000,
            },
        )
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
