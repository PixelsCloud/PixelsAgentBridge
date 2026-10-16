use super::*;
use pab_desktop_media::NativeViewer;
use pab_protocol::DesktopOpened;
use std::time::Duration;
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::{mpsc, watch};

#[derive(Clone, Default, Serialize)]
pub struct ViewerStatus {
    phase: String,
    error: Option<String>,
    opened: Option<DesktopOpened>,
    input_ready: bool,
    input_enabled: bool,
}
enum Command {
    Screen(String),
    Fps(u16),
    Retry,
    Input(bool),
}
struct Owner {
    code: DeviceCode,
    account: String,
    runtime: Arc<BridgeRuntime>,
    stop: watch::Sender<bool>,
    commands: mpsc::Sender<Command>,
    requests: Mutex<Option<mpsc::Receiver<Command>>>,
    status: watch::Sender<ViewerStatus>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
#[derive(Default)]
pub struct RemoteDesktopState {
    windows: Mutex<HashMap<String, Arc<Owner>>>,
}
fn account() -> Result<String, String> {
    pab_agent_core::account::AccountStore::from_env()
        .and_then(|s| s.read())
        .map_err(|e| e.to_string())?
        .user
        .map(|u| u.id.to_string())
        .ok_or_else(|| "login required to view a remote computer".into())
}
impl RemoteDesktopState {
    async fn owner(&self, window: &WebviewWindow) -> Result<Arc<Owner>, String> {
        let owner = self
            .windows
            .lock()
            .await
            .get(window.label())
            .cloned()
            .ok_or("remote desktop window is closed")?;
        if owner.account != account()? {
            return Err("account changed; reopen remote desktop".into());
        }
        Ok(owner)
    }
    async fn close(&self, label: &str) {
        let owner = self.windows.lock().await.remove(label);
        if let Some(owner) = owner {
            owner.stop.send_replace(true);
            if let Some(mut task) = owner.task.lock().await.take() {
                if tokio::time::timeout(Duration::from_secs(5), &mut task)
                    .await
                    .is_err()
                {
                    task.abort();
                }
            }
        }
    }
}
#[tauri::command]
pub async fn desktop_viewer_window(
    app: tauri::AppHandle,
    window: WebviewWindow,
    state: State<'_, OperatorState>,
    viewers: State<'_, RemoteDesktopState>,
    code: String,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("only the main window can open a remote desktop".into());
    }
    if !cfg!(windows) {
        return Err("native remote desktop viewer is not available on this platform yet".into());
    }
    let account = account()?;
    let code = parse_code(&code)?;
    let runtime = state.runtime().await?;
    let saved = runtime
        .remembered_devices()
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|d| d.code == code)
        .ok_or("device is not in the added device list")?;
    let mut owners = viewers.windows.lock().await;
    for (label, owner) in owners.iter() {
        if owner.code == code
            && owner.account == account
            && Arc::ptr_eq(&owner.runtime, &runtime)
            && !*owner.stop.borrow()
        {
            if let Some(window) = app.get_webview_window(label) {
                let _ = window.unminimize();
                return window.set_focus().map_err(|e| e.to_string());
            }
        }
    }
    let label = format!("desktop-viewer-{}", RequestId::new());
    let (stop, _) = watch::channel(false);
    let (commands, requests) = mpsc::channel(2);
    let (status, _) = watch::channel(ViewerStatus {
        phase: "ready".into(),
        ..Default::default()
    });
    let owner = Arc::new(Owner {
        code,
        account,
        runtime,
        stop,
        commands,
        requests: Mutex::new(Some(requests)),
        status,
        task: Mutex::new(None),
    });
    owners.insert(label.clone(), owner);
    let title = format!(
        "Pixels · {} · {code}",
        if saved.alias.is_empty() {
            "远程桌面"
        } else {
            &saved.alias
        }
    );
    match WebviewWindowBuilder::new(
        &app,
        &label,
        WebviewUrl::App(format!("index.html?desktopViewer={label}").into()),
    )
    .title(title)
    .inner_size(1100.0, 760.0)
    .min_inner_size(640.0, 400.0)
    .resizable(true)
    .build()
    {
        Ok(created) => {
            let label = label.clone();
            created.on_window_event(move |event| {
                if matches!(event, tauri::WindowEvent::Destroyed) {
                    let app = app.clone();
                    let label = label.clone();
                    tauri::async_runtime::spawn(async move {
                        app.state::<RemoteDesktopState>().close(&label).await;
                    });
                }
            });
            Ok(())
        }
        Err(error) => {
            owners.remove(&label);
            Err(error.to_string())
        }
    }
}
#[tauri::command]
pub async fn desktop_viewer_start(
    window: WebviewWindow,
    state: State<'_, RemoteDesktopState>,
) -> Result<ViewerStatus, String> {
    let owner = state.owner(&window).await?;
    let mut task = owner.task.lock().await;
    if task.is_none() {
        #[cfg(windows)]
        let parent = window.hwnd().map_err(|e| e.to_string())?.0 as usize;
        #[cfg(not(windows))]
        let parent = 0usize;
        let requests = owner
            .requests
            .lock()
            .await
            .take()
            .ok_or("viewer already started")?;
        let owner = owner.clone();
        *task = Some(tokio::spawn(async move {
            run(window, owner, requests, parent).await;
        }));
    }
    Ok(owner.status.borrow().clone())
}
#[tauri::command]
pub async fn desktop_viewer_control(
    window: WebviewWindow,
    state: State<'_, RemoteDesktopState>,
    screen: Option<String>,
    fps: Option<u16>,
    retry: Option<bool>,
    input_enabled: Option<bool>,
) -> Result<(), String> {
    let owner = state.owner(&window).await?;
    let command = match (screen, fps, retry, input_enabled) {
        (Some(screen), None, None, None) => {
            pab_protocol::validate_desktop_screen_id(&screen).map_err(str::to_owned)?;
            Command::Screen(screen)
        }
        (None, Some(fps @ (15 | 30)), None, None) => Command::Fps(fps),
        (None, None, Some(true), None) => Command::Retry,
        (None, None, None, Some(enabled)) => Command::Input(enabled),
        _ => return Err("invalid remote desktop command".into()),
    };
    owner
        .commands
        .try_send(command)
        .map_err(|_| "remote desktop is busy or closed; retry shortly".into())
}
fn publish(
    window: &WebviewWindow,
    owner: &Owner,
    phase: &str,
    opened: Option<DesktopOpened>,
    error: Option<String>,
) {
    let status = ViewerStatus {
        phase: phase.into(),
        opened,
        error,
        input_ready: false,
        input_enabled: false,
    };
    owner.status.send_replace(status.clone());
    let _ = window.emit_to(window.label(), "desktop-viewer-status", status);
}
fn publish_input(window: &WebviewWindow, owner: &Owner, ready: bool, enabled: bool) {
    let mut status = owner.status.borrow().clone();
    status.input_ready = ready;
    status.input_enabled = enabled;
    owner.status.send_replace(status.clone());
    let _ = window.emit_to(window.label(), "desktop-viewer-status", status);
}
async fn run(
    window: WebviewWindow,
    owner: Arc<Owner>,
    mut commands: mpsc::Receiver<Command>,
    parent: usize,
) {
    let mut stopped = owner.stop.subscribe();
    let mut accounts = pab_agent_core::account::account_changes();
    let cancelled = async {
        loop {
            if *stopped.borrow() || account().ok().as_ref() != Some(&owner.account) {
                break;
            }
            tokio::select! { _ = stopped.changed() => {}, result = accounts.changed() => if result.is_err() { break; } }
        }
    };
    let work = async {
        let mut fps = 15;
        let mut screen = None;
        loop {
            publish(&window, &owner, "connecting", None, None);
            let result = stream(
                &window,
                &owner,
                &mut commands,
                parent,
                &mut fps,
                &mut screen,
            )
            .await;
            match result {
                Ok(true) => continue,
                Ok(false) => break,
                Err(error) => {
                    publish(&window, &owner, "failed", None, Some(error));
                    loop {
                        match commands.recv().await {
                            Some(Command::Retry) => {
                                screen = None;
                                break;
                            }
                            Some(Command::Fps(value)) => {
                                fps = value;
                                break;
                            }
                            Some(Command::Screen(value)) => {
                                screen = Some(value);
                                break;
                            }
                            Some(Command::Input(_)) => continue,
                            None => return,
                        }
                    }
                }
            }
        }
    };
    tokio::select! { _ = work => {}, _ = cancelled => {} }
    owner.stop.send_replace(true);
    publish(&window, &owner, "ended", None, None);
}
async fn stream(
    window: &WebviewWindow,
    owner: &Owner,
    commands: &mut mpsc::Receiver<Command>,
    parent: usize,
    fps: &mut u16,
    screen: &mut Option<String>,
) -> Result<bool, String> {
    let device = owner
        .runtime
        .resolve_device_code(owner.code)
        .await
        .map_err(|e| e.to_string())?;
    let mut session = owner
        .runtime
        .open_remote_desktop(device, screen.clone(), *fps)
        .await
        .map_err(|e| e.to_string())?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let prefix = vec!["--media-worker".into()];
    loop {
        let (mut viewer, mut native_input) = tokio::time::timeout(
            Duration::from_secs(10),
            NativeViewer::open_interactive(&executable, &prefix, &session.opened().config, parent),
        )
        .await
        .map_err(|_| "native viewer startup timed out")?
        .map_err(|e| e.to_string())?;
        publish(
            window,
            owner,
            "viewing",
            Some(session.opened().clone()),
            None,
        );
        let control = session.input_control();
        let supported = session.opened().input_supported;
        let (ready, mut ready_rx) = watch::channel(false);
        // These are independent futures for the entire generation. Mouse/keys
        // and toolbar commands never cancel a partially-read compressed frame.
        let video = async {
            loop {
                let frame = session
                    .next_frame_when_changed()
                    .await
                    .map_err(|e| e.to_string())?;
                tokio::time::timeout(Duration::from_secs(5), viewer.present(&frame))
                    .await
                    .map_err(|_| "native renderer timed out")?
                    .map_err(|e| e.to_string())?;
                session
                    .report_presented(&frame.header)
                    .map_err(|e| e.to_string())?;
                if !*ready.borrow() {
                    ready.send_replace(true);
                    publish_input(window, owner, supported, false);
                }
            }
            #[allow(unreachable_code)]
            Ok::<(), String>(())
        };
        let interaction = async {
            let mut enabled = false;
            loop {
                tokio::select! {
                    biased;
                    command = commands.recv() => match command {
                        Some(Command::Input(value)) => {
                            if value && (!supported || !*ready_rx.borrow_and_update()) {
                                publish_input(window, owner, supported && *ready_rx.borrow(), false);
                                continue;
                            }
                            // Drain old local events before changing remote ownership.
                            tokio::time::timeout(Duration::from_secs(3), native_input.set_enabled(false)).await
                                .map_err(|_| "native input mode timed out")?.map_err(|e| e.to_string())?;
                            control.set_enabled(value).await.map_err(|e| e.to_string())?;
                            if value {
                                tokio::time::timeout(Duration::from_secs(3), native_input.set_enabled(true)).await
                                    .map_err(|_| "native input mode timed out")?.map_err(|e| e.to_string())?;
                            }
                            enabled = value;
                            publish_input(window, owner, supported && *ready_rx.borrow(), enabled);
                        }
                        other => return Ok::<_, String>(other),
                    },
                    event = native_input.next() => {
                        let event = event.map_err(|e| e.to_string())?;
                        if enabled { control.send(event).await.map_err(|e| e.to_string())?; }
                    }
                }
            }
        };
        let command = tokio::select! {
            result = video => { result?; None },
            result = interaction => result?,
        };
        drop(viewer);
        match command {
            Some(Command::Screen(id)) => {
                publish(
                    window,
                    owner,
                    "switching",
                    Some(session.opened().clone()),
                    None,
                );
                session
                    .select_screen(id.clone())
                    .await
                    .map_err(|e| e.to_string())?;
                *screen = Some(id);
            }
            Some(Command::Fps(value)) => {
                *fps = value;
                session.close().await.map_err(|e| e.to_string())?;
                return Ok(true);
            }
            Some(Command::Retry) => {
                session.close().await.map_err(|e| e.to_string())?;
                return Ok(true);
            }
            Some(Command::Input(_)) => {
                unreachable!("input commands are handled within the generation")
            }
            None => {
                let _ = session.close().await;
                return Ok(false);
            }
        }
    }
}
