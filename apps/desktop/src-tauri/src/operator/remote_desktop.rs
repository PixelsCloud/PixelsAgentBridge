use super::*;
#[cfg(target_os = "linux")]
use pab_desktop_media::LinuxNativeViewer as NativeViewer;
#[cfg(target_os = "macos")]
use pab_desktop_media::MacNativeViewer as NativeViewer;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use pab_desktop_media::NativeViewer;
use pab_protocol::DesktopOpened;
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::{mpsc, watch};
mod recovery;

#[derive(Clone, Default, Serialize)]
pub struct ViewerStatus {
    phase: String,
    error: Option<String>,
    opened: Option<DesktopOpened>,
    input_ready: bool,
    frame_ready: bool,
    input_enabled: bool,
    decoder: Option<&'static str>,
}
enum Command {
    Screen(String),
    Fps(u16),
    Retry,
    Input(bool, InputGeneration),
    SecureAttention(InputGeneration),
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct InputGeneration {
    session: RequestId,
    generation: u64,
}
impl From<&DesktopOpened> for InputGeneration {
    fn from(opened: &DesktopOpened) -> Self {
        Self {
            session: opened.config.session_id,
            generation: opened.config.generation,
        }
    }
}
struct StreamFailure {
    message: String,
    recoverable: bool,
    renderer: bool,
}
impl StreamFailure {
    fn renderer(error: impl std::fmt::Display) -> Self {
        Self {
            message: error.to_string(),
            recoverable: false,
            renderer: true,
        }
    }
}
impl From<String> for StreamFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            recoverable: false,
            renderer: false,
        }
    }
}
impl From<&str> for StreamFailure {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
impl From<pab_bridge::RemoteDesktopError> for StreamFailure {
    fn from(error: pab_bridge::RemoteDesktopError) -> Self {
        Self {
            recoverable: error.is_recoverable_media(),
            message: error.to_string(),
            renderer: false,
        }
    }
}
impl From<pab_bridge::RuntimeError> for StreamFailure {
    fn from(error: pab_bridge::RuntimeError) -> Self {
        let recoverable = match &error {
            pab_bridge::RuntimeError::Bridge(error) => {
                error.is_recoverable_connection()
                    || matches!(error, pab_bridge::BridgeError::RemoteDesktop(media)
                        if media.is_recoverable_media())
            }
            _ => false,
        };
        Self {
            recoverable,
            message: error.to_string(),
            renderer: false,
        }
    }
}
struct Owner {
    code: DeviceCode,
    initial_screen: Option<String>,
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
    let snapshot = pab_agent_core::account::AccountStore::from_env()
        .and_then(|s| s.read())
        .map_err(|e| e.to_string())?;
    snapshot
        .user
        .map(|u| format!("{}:{}", u.id, snapshot.revision))
        .ok_or_else(|| "login required to view a remote computer".into())
}
impl RemoteDesktopState {
    pub(crate) async fn assistance_viewing(&self, label: &str, code: DeviceCode) -> bool {
        let owners = self.windows.lock().await;
        owners.get(label).is_some_and(|owner| {
            owner.code == code
                && account().ok().as_ref() == Some(&owner.account)
                && !*owner.stop.borrow()
                && {
                    let status = owner.status.borrow();
                    status.phase == "viewing" && status.frame_ready
                }
        })
    }
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
    open_window(app, &state, &viewers, code, None, false)
        .await
        .map(|_| ())
}
pub(crate) async fn open_window(
    app: tauri::AppHandle,
    state: &OperatorState,
    viewers: &RemoteDesktopState,
    code: String,
    initial_screen: Option<String>,
    for_assistance: bool,
) -> Result<String, String> {
    if !cfg!(any(windows, target_os = "macos", target_os = "linux")) {
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
        .find(|d| d.code == code);
    if saved.is_none() && !for_assistance {
        return Err("device is not in the added device list".into());
    }
    // An authenticated MCP request need not add a permanent catalog entry.
    // The viewer still establishes its own existing authenticated connection.
    let mut owners = viewers.windows.lock().await;
    for (label, owner) in owners.iter() {
        if owner.code == code
            && owner.account == account
            && Arc::ptr_eq(&owner.runtime, &runtime)
            && !*owner.stop.borrow()
        {
            if let Some(window) = app.get_webview_window(label) {
                if for_assistance {
                    let status = owner.status.borrow().clone();
                    if status.input_enabled {
                        if let Some(opened) = status.opened {
                            owner
                                .commands
                                .try_send(Command::Input(false, InputGeneration::from(&opened)))
                                .map_err(|_| "viewer is busy; retry opening assistance")?;
                        }
                    }
                }
                if let Some(screen) = initial_screen {
                    owner
                        .commands
                        .try_send(Command::Screen(screen))
                        .map_err(|_| "viewer is busy; retry opening assistance")?;
                }
                let _ = window.unminimize();
                window.set_focus().map_err(|e| e.to_string())?;
                return Ok(label.clone());
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
        initial_screen,
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
        saved
            .as_ref()
            .map(|s| s.alias.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("远程桌面")
    );
    let builder = WebviewWindowBuilder::new(
        &app,
        &label,
        WebviewUrl::App(format!("index.html?desktopViewer={label}").into()),
    )
    .title(title)
    .inner_size(1100.0, 760.0)
    .min_inner_size(640.0, 400.0)
    .resizable(true);
    // All WebViews sharing the acceptance profile must use identical WebView2
    // environment arguments, as the terminal and main window already do.
    #[cfg(all(windows, debug_assertions))]
    let builder = if let Some(arguments) = super::desktop_terminal::acceptance_browser_arguments() {
        builder.additional_browser_args(arguments)
    } else {
        builder
    };
    match builder.build() {
        Ok(created) => {
            let closed_label = label.clone();
            created.on_window_event(move |event| {
                if matches!(event, tauri::WindowEvent::Destroyed) {
                    let app = app.clone();
                    let label = closed_label.clone();
                    tauri::async_runtime::spawn(async move {
                        app.state::<RemoteDesktopState>().close(&label).await;
                    });
                }
            });
            Ok(label)
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
    secure_attention: Option<bool>,
) -> Result<(), String> {
    let owner = state.owner(&window).await?;
    let status = owner.status.borrow().clone();
    let input_generation = || {
        status
            .opened
            .as_ref()
            .filter(|_| status.phase == "viewing" && status.input_ready)
            .map(InputGeneration::from)
            .ok_or_else(|| "remote desktop input is not ready".to_owned())
    };
    let command = match (screen, fps, retry, input_enabled, secure_attention) {
        (Some(screen), None, None, None, None) => {
            pab_protocol::validate_desktop_screen_id(&screen).map_err(str::to_owned)?;
            Command::Screen(screen)
        }
        (None, Some(fps @ (15 | 30)), None, None, None) => Command::Fps(fps),
        (None, None, Some(true), None, None) => Command::Retry,
        (None, None, None, Some(enabled), None) => Command::Input(enabled, input_generation()?),
        (None, None, None, None, Some(true)) if status.input_enabled => {
            Command::SecureAttention(input_generation()?)
        }
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
        frame_ready: false,
        input_enabled: false,
        decoder: None,
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
        // Account notifications are process-local hints; another Desktop/CLI
        // can change the protected store without notifying this process.
        let mut poll = tokio::time::interval(Duration::from_secs(3));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if *stopped.borrow() || account().ok().as_ref() != Some(&owner.account) {
                break;
            }
            tokio::select! { _ = stopped.changed() => {}, _ = poll.tick() => {}, result = accounts.changed() => if result.is_err() { break; } }
        }
    };
    let work = async {
        let mut fps = 15;
        let mut screen = owner.initial_screen.clone();
        let mut failures = 0u32;
        let mut first_failure = None;
        let mut software_decoder = false;
        loop {
            // Input and SAS belong to the previous generation. Never replay
            // queued actions into a newly opened login desktop.
            while let Ok(command) = commands.try_recv() {
                match command {
                    Command::Screen(id) => screen = Some(id),
                    Command::Fps(value) => fps = value,
                    _ => {}
                }
            }
            publish(
                &window,
                &owner,
                if failures == 0 {
                    "connecting"
                } else {
                    "reconnecting"
                },
                None,
                None,
            );
            let mut first_presented = None;
            let result = stream(
                &window,
                &owner,
                &mut commands,
                parent,
                &mut fps,
                &mut screen,
                &mut first_presented,
                software_decoder,
            )
            .await;
            match result {
                Ok(true) => continue,
                Ok(false) => break,
                Err(error) => {
                    if error.renderer && !software_decoder {
                        // Decode/render failure gets one software attempt in a
                        // fresh, view-only session. Network/auth failures never
                        // change codec selection or replay previous input.
                        software_decoder = true;
                        failures += 1;
                        publish(&window, &owner, "reconnecting", None, None);
                        continue;
                    }
                    // A single frame followed by failure must not create an
                    // endless restart loop. Reset only after a stable interval.
                    if first_presented
                        .is_some_and(|at: Instant| at.elapsed() >= Duration::from_secs(5))
                    {
                        failures = 0;
                        first_failure = None;
                    }
                    if error.recoverable
                        && let Some(delay) =
                            recovery::retry_delay(&mut first_failure, failures, Instant::now())
                    {
                        failures = failures.saturating_add(1);
                        publish(&window, &owner, "reconnecting", None, None);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    publish(&window, &owner, "failed", None, Some(error.message));
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
                            Some(Command::Input(..) | Command::SecureAttention(_)) => continue,
                            None => return,
                        }
                    }
                    failures = 0;
                    first_failure = None;
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
    first_presented: &mut Option<Instant>,
    software_decoder: bool,
) -> Result<bool, StreamFailure> {
    let device = owner
        .runtime
        .resolve_device_code(owner.code)
        .await
        .map_err(StreamFailure::from)?;
    let opened = owner
        .runtime
        .open_remote_desktop(device, screen.clone(), *fps)
        .await;
    let mut session = match opened {
        Err(pab_bridge::RuntimeError::Bridge(pab_bridge::BridgeError::RemoteDesktop(
            pab_bridge::RemoteDesktopError::Rejected {
                code: pab_protocol::DesktopErrorCode::NotFound,
                ..
            },
        ))) if screen.is_some() => {
            // Open's NotFound means the selected output disappeared. A fresh
            // primary-screen session is view-only; never replay old input.
            *screen = None;
            owner
                .runtime
                .open_remote_desktop(device, None, *fps)
                .await
                .map_err(StreamFailure::from)?
        }
        other => other.map_err(StreamFailure::from)?,
    };
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let prefix = vec!["--media-worker".into()];
    loop {
        let started = tokio::time::timeout(
            Duration::from_secs(10),
            open_native_viewer(
                window,
                &executable,
                &prefix,
                &session.opened().config,
                parent,
                software_decoder,
            ),
        )
        .await
        .map_err(|_| StreamFailure::renderer("native viewer startup timed out"))
        .and_then(|result| result.map_err(StreamFailure::renderer));
        let (mut viewer, mut native_input) = match started {
            Ok(viewer) => viewer,
            Err(error) => {
                let _ = tokio::time::timeout(Duration::from_secs(2), session.close()).await;
                return Err(error);
            }
        };
        // The cursor owns a separate remote stream and native submitter. It
        // must stay alive across a pending video read on a static desktop.
        #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
        let cursor_source = if session.supports_cursor() {
            match session.open_cursor().await {
                Ok(source) => Some(source),
                Err(_) => {
                    tracing::debug!("independent remote cursor unavailable for this generation");
                    None
                }
            }
        } else {
            None
        };
        #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
        let cursor_renderer = if cursor_source.is_some() {
            Some(viewer.cursor_renderer().map_err(StreamFailure::renderer)?)
        } else {
            None
        };
        #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
        let cursor_work = async move {
            if let (Some(mut source), Some(mut renderer)) = (cursor_source, cursor_renderer) {
                loop {
                    let update = match source.next_update().await {
                        Ok(update) => update,
                        Err(_) => {
                            // Remote cursor failure is independent of video and
                            // input. Erase its last image and leave the viewer.
                            tokio::time::timeout(Duration::from_secs(5), renderer.clear())
                                .await
                                .map_err(|_| {
                                    StreamFailure::renderer("native cursor clear timed out")
                                })?
                                .map_err(StreamFailure::renderer)?;
                            break;
                        }
                    };
                    // A local partial pipe write cannot be reused. Such an
                    // error retires this native generation, like video IO.
                    tokio::time::timeout(Duration::from_secs(5), renderer.submit(&update))
                        .await
                        .map_err(|_| StreamFailure::renderer("native cursor submission timed out"))?
                        .map_err(StreamFailure::renderer)?;
                }
            }
            std::future::pending::<Result<(), StreamFailure>>().await
        };
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        let cursor_work = std::future::pending::<Result<(), StreamFailure>>();
        publish(
            window,
            owner,
            "viewing",
            Some(session.opened().clone()),
            None,
        );
        let control = session.input_control();
        let supported = session.opened().input_supported;
        let input_generation = InputGeneration::from(session.opened());
        let (ready, mut ready_rx) = watch::channel(false);
        // These are independent futures for the entire generation. Mouse/keys
        // and toolbar commands never cancel a partially-read compressed frame.
        let mut presentations = viewer.presentations();
        let acknowledgements = session.presentation_control();
        let video = async {
            let submit = async {
                loop {
                    let frame = session
                        .next_frame_when_changed()
                        .await
                        .map_err(StreamFailure::from)?;
                    tokio::time::timeout(Duration::from_secs(5), viewer.submit(&frame))
                        .await
                        .map_err(|_| {
                            StreamFailure::renderer("native decoder submission timed out")
                        })?
                        .map_err(StreamFailure::renderer)?;
                }
                #[allow(unreachable_code)]
                Ok::<(), StreamFailure>(())
            };
            let output = async {
                loop {
                    let header = presentations
                        .next()
                        .await
                        .map_err(StreamFailure::renderer)?;
                    acknowledgements
                        .report_presented(&header)
                        .map_err(StreamFailure::from)?;
                    if !*ready.borrow() {
                        first_presented.get_or_insert_with(Instant::now);
                        ready.send_replace(true);
                        owner.status.send_modify(|status| {
                            status.frame_ready = true;
                            status.decoder = presentations.decoder();
                        });
                        publish_input(window, owner, supported, false);
                    }
                }
                #[allow(unreachable_code)]
                Ok::<(), StreamFailure>(())
            };
            tokio::try_join!(submit, output)?;
            Ok::<(), StreamFailure>(())
        };
        let interaction = async {
            let mut enabled = false;
            loop {
                tokio::select! {
                    biased;
                    command = commands.recv() => match command {
                        Some(Command::Input(value, target)) if target == input_generation => {
                            if value && (!supported || !*ready_rx.borrow_and_update()) {
                                publish_input(window, owner, supported && *ready_rx.borrow(), false);
                                continue;
                            }
                            // Drain old local events before changing remote ownership.
                            tokio::time::timeout(Duration::from_secs(3), native_input.set_enabled(false)).await
                                .map_err(|_| StreamFailure::renderer("native input mode timed out"))?.map_err(StreamFailure::renderer)?;
                            control.set_enabled(value).await.map_err(StreamFailure::from)?;
                            if value {
                                tokio::time::timeout(Duration::from_secs(3), native_input.set_enabled(true)).await
                                    .map_err(|_| StreamFailure::renderer("native input mode timed out"))?.map_err(StreamFailure::renderer)?;
                            }
                            enabled = value;
                            publish_input(window, owner, supported && *ready_rx.borrow(), enabled);
                        }
                        Some(Command::SecureAttention(target)) if target == input_generation => {
                            if !enabled { continue; }
                            // Stop local collection and retire held remote keys
                            // before the system changes desktops. SAS is sent
                            // once through the existing authenticated service.
                            tokio::time::timeout(Duration::from_secs(3), native_input.set_enabled(false)).await
                                .map_err(|_| StreamFailure::renderer("native input mode timed out"))?.map_err(StreamFailure::renderer)?;
                            control.set_enabled(false).await.map_err(StreamFailure::from)?;
                            publish_input(window, owner, false, false);
                            owner.runtime.desktop_input(device, pab_protocol::DesktopInputEvent::SecureAttention)
                                .await.map_err(|e| e.to_string())?;
                            return Ok(Some(Command::Retry));
                        }
                        Some(Command::Input(..) | Command::SecureAttention(_)) => continue,
                        other => return Ok::<_, StreamFailure>(other),
                    },
                    event = native_input.next() => {
                        let event = event.map_err(StreamFailure::renderer)?;
                        if enabled { control.send(event).await.map_err(StreamFailure::from)?; }
                    }
                }
            }
        };
        let outcome = tokio::select! {
            result = video => result.map(|_| None),
            result = interaction => result,
            result = cursor_work => result.map(|_| None),
        };
        drop(viewer);
        let command = match outcome {
            Ok(command) => command,
            Err(error) => {
                // Retire this owner's remote slot before reopening. Dropping
                // alone schedules cleanup asynchronously and can race Busy.
                let _ = tokio::time::timeout(Duration::from_secs(2), session.close()).await;
                return Err(error);
            }
        };
        match command {
            Some(Command::Screen(id)) => {
                publish(
                    window,
                    owner,
                    "switching",
                    Some(session.opened().clone()),
                    None,
                );
                if let Err(error) = session.select_screen(id.clone()).await {
                    let missing = matches!(
                        error,
                        pab_bridge::RemoteDesktopError::Rejected {
                            code: pab_protocol::DesktopErrorCode::NotFound,
                            ..
                        }
                    );
                    let _ = tokio::time::timeout(Duration::from_secs(2), session.close()).await;
                    if missing {
                        *screen = None;
                        return Ok(true);
                    }
                    return Err(error.into());
                }
                *screen = Some(id);
            }
            Some(Command::Fps(value)) => {
                *fps = value;
                session.close().await.map_err(StreamFailure::from)?;
                return Ok(true);
            }
            Some(Command::Retry) => {
                session.close().await.map_err(StreamFailure::from)?;
                return Ok(true);
            }
            Some(Command::Input(..) | Command::SecureAttention(_)) => {
                unreachable!("input commands are handled within the generation")
            }
            None => {
                let _ = session.close().await;
                return Ok(false);
            }
        }
    }
}

async fn open_native_viewer(
    window: &WebviewWindow,
    path: &std::path::PathBuf,
    prefix: &[String],
    config: &pab_protocol::DesktopVideoConfig,
    parent: usize,
    software: bool,
) -> Result<
    (NativeViewer, pab_desktop_media::NativeViewerInput),
    Box<dyn std::error::Error + Send + Sync>,
> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = window;
        NativeViewer::open_interactive_with_software(path, prefix, config, parent, software).await
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let _ = (path, prefix, parent);
        let app = window.app_handle().clone();
        let dispatch: pab_desktop_media::MainThreadDispatch = Arc::new(move |work| {
            app.run_on_main_thread(work)
                .map_err(|error| error.to_string())
        });
        let host = window.clone();
        let config = config.clone();
        let runtime = tokio::runtime::Handle::current();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        window.app_handle().run_on_main_thread(move || {
            if sender.is_closed() {
                return;
            }
            let _entered = runtime.enter();
            #[cfg(target_os = "macos")]
            let result = host
                .ns_window()
                .map_err(|error| {
                    Box::<dyn std::error::Error + Send + Sync>::from(error.to_string())
                })
                .and_then(|parent| {
                    // SAFETY: get the live NSWindow on its owning main thread;
                    // no raw address comes from a webview or remote message.
                    unsafe { NativeViewer::create(parent, &config, software, dispatch) }
                });
            #[cfg(target_os = "linux")]
            let result = {
                use gtk::glib::object::ObjectType;
                let _ = software; // Linux currently uses the bundled software decoder.
                host.gtk_window()
                    .map_err(|error| {
                        Box::<dyn std::error::Error + Send + Sync>::from(error.to_string())
                    })
                    .and_then(|parent| {
                        // SAFETY: this live host-owned GtkWindow is obtained on
                        // the GTK main thread; its pointer never crosses IPC.
                        unsafe {
                            NativeViewer::create_interactive(
                                parent.as_ptr().cast(),
                                &config,
                                dispatch,
                            )
                        }
                    })
            };
            // If startup was cancelled while queued, dropping this result also
            // retires its native view and input sink on the owning thread.
            let _ = sender.send(result);
        })?;
        receiver
            .await
            .map_err(|_| "native view creation interrupted")?
    }
}
