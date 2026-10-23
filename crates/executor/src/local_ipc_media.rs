//! A dedicated, authenticated helper socket for compressed desktop media.
//! No frame enters the window/screenshot/application request queue. Requests
//! are pull based (one outstanding packet); cancellation retires this socket
//! and its source, never the Executor's shared device connection.
use super::*;
use pab_protocol::{
    DESKTOP_CONTROL_BYTES, DESKTOP_MAX_FRAME_BYTES, DESKTOP_VIDEO_CHUNK_BYTES, DesktopScreen,
    DesktopVideoConfig, validate_desktop_screens,
};
use pab_remote_desktop::{DesktopBackend, DesktopError, DesktopSource, MediaFrame, MediaFuture};
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use tokio::sync::Notify;
#[path = "local_ipc_media_input.rs"]
pub(super) mod input;
#[path = "local_ipc_media_cursor.rs"]
pub(super) mod cursor;

const VERSION: u16 = 1;
const ENCODING_BUDGET_VERSION: u16 = 1;
static PROVIDERS: OnceLock<Mutex<Vec<Arc<Provider>>>> = OnceLock::new();
static CHANGED: Notify = Notify::const_new();
fn providers() -> &'static Mutex<Vec<Arc<Provider>>> {
    PROVIDERS.get_or_init(|| Mutex::new(Vec::new()))
}
struct Provider {
    id: u64,
    supervised: bool,
    sender: mpsc::Sender<Request>,
    retired: AtomicBool,
    identity: Arc<helper_identity::VerifiedMediaHelper>,
    active_config: Mutex<Option<DesktopVideoConfig>>,
    input_supported: bool,
    clipboard_supported: bool,
    input: Mutex<Option<Arc<input::InputProvider>>>,
    cursor_supported: bool,
    encoding_budget_supported: bool,
    timings_supported: bool,
    cursor: Mutex<Option<Arc<cursor::CursorProvider>>>,
    ended: tokio::sync::watch::Sender<bool>,
}
struct Registration(u64);
impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut providers) = providers().lock() {
            providers.retain(|p| {
                if p.id == self.0 {
                    p.retired.store(true, Ordering::Release);
                    p.ended.send_replace(true);
                    false
                } else {
                    true
                }
            });
        }
        CHANGED.notify_waiters();
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Screens,
    Start { config: DesktopVideoConfig },
    Next,
    NextTimed,
    EncodingBudget { bits_per_second: u32 },
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    Screens {
        screens: Vec<DesktopScreen>,
    },
    Started,
    EncodingBudget { bits_per_second: Option<u32> },
    Frame {
        bytes: usize,
        capture_time_us: u64,
        keyframe: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timings: Option<pab_protocol::DesktopCaptureTimings>,
    },
    Error {
        message: String,
    },
}
enum Response {
    Screens(Vec<DesktopScreen>),
    Started,
    EncodingBudget(Option<u32>),
    Frame(MediaFrame, Option<pab_protocol::DesktopCaptureTimings>),
}
struct Request {
    operation: Operation,
    // Held by the returned source. Closing it retires the registration even
    // when nobody is currently reading a frame.
    stop: Option<oneshot::Receiver<()>>,
    reply: oneshot::Sender<Result<Response, LocalIpcError>>,
}
fn unavailable(error: impl std::fmt::Display) -> DesktopError {
    DesktopError::Rejected {
        code: pab_protocol::DesktopErrorCode::MediaUnavailable,
        message: error.to_string(),
    }
}
async fn provider() -> Result<Arc<Provider>, LocalIpcError> {
    tokio::time::timeout(AUTH_TIMEOUT, async {
        loop {
            // Register before checking: a helper appearing here cannot be lost.
            let changed = CHANGED.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(provider) = providers()
                .lock()
                .map_err(|_| LocalIpcError::Protocol)?
                .iter()
                .enumerate()
                .filter(|(_, p)| !p.sender.is_closed() && !p.retired.load(Ordering::Acquire))
                // Preserve actual registration order within each class, even
                // when concurrent identity challenges finish out of ID order.
                .max_by_key(|(order, p)| (p.supervised, *order))
                .map(|(_, p)| p.clone())
            {
                return Ok(provider);
            }
            changed.await;
        }
    })
    .await
    .map_err(|_| LocalIpcError::Timeout)?
}
async fn request(
    provider: &Provider,
    operation: Operation,
    stop: Option<oneshot::Receiver<()>>,
) -> Result<Response, LocalIpcError> {
    let (reply, result) = oneshot::channel();
    provider
        .sender
        .send(Request {
            operation,
            stop,
            reply,
        })
        .await
        .map_err(|_| LocalIpcError::Closed)?;
    result.await.map_err(|_| LocalIpcError::Closed)?
}

/// Baseline video/input support is independent of transient helper availability.
/// Optional cursor support follows the selected helper's version. Failure here
/// is a media error, not a device connection error.
pub struct HelperMediaBackend;
impl DesktopBackend for HelperMediaBackend {
    fn supports_clipboard(&self) -> bool {
        providers().lock().ok().and_then(|providers| providers.iter()
            .filter(|provider| !provider.sender.is_closed() && !provider.retired.load(Ordering::Acquire))
            .max_by_key(|provider| (provider.active_config.lock().ok().is_some_and(|config| config.is_some()), provider.supervised, provider.id))
            .map(|provider| provider.clipboard_supported)).unwrap_or(false)
    }
    fn clipboard(&self, config: DesktopVideoConfig, operation: pab_protocol::DesktopClipboardOperation) -> MediaFuture<'_, pab_protocol::DesktopClipboardSnapshot> {
        Box::pin(input::clipboard(config, operation))
    }
    fn supports_cursor(&self) -> bool {
        // Keep an active capture's capability when another helper registers.
        // Otherwise match capture routing, including supervised preference.
        // An older/embedded-only helper must not advertise a separate cursor.
        providers()
            .lock()
            .ok()
            .and_then(|providers| {
                providers.iter().enumerate()
                    .filter(|(_, p)| !p.sender.is_closed() && !p.retired.load(Ordering::Acquire))
                    .max_by_key(|(order, p)| (
                        p.active_config.lock().ok().is_some_and(|config| config.is_some()),
                        p.supervised,
                        *order,
                    ))
                    .map(|(_, p)| p.cursor_supported)
            })
            .unwrap_or(false)
    }
    fn cursor(
        &self,
        config: DesktopVideoConfig,
    ) -> MediaFuture<'_, Box<dyn pab_remote_desktop::DesktopCursorSource>> {
        Box::pin(cursor::open(config))
    }
    fn supports_input(&self) -> bool { cfg!(any(windows, target_os = "macos", target_os = "linux")) }
    fn input(
        &self,
        config: DesktopVideoConfig,
    ) -> MediaFuture<'_, Box<dyn pab_remote_desktop::DesktopInputSink>> {
        Box::pin(input::open(config))
    }
    fn screens(&self) -> MediaFuture<'_, Vec<DesktopScreen>> {
        Box::pin(async {
            let provider = provider().await.map_err(unavailable)?;
            match request(&provider, Operation::Screens, None)
                .await
                .map_err(unavailable)?
            {
                Response::Screens(screens) => Ok(screens),
                _ => Err(unavailable("invalid media screen reply")),
            }
        })
    }
    fn start(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopSource>> {
        Box::pin(async {
            config.validate().map_err(unavailable)?;
            let provider = provider().await.map_err(unavailable)?;
            let (stop, stopped) = oneshot::channel();
            match request(
                &provider,
                Operation::Start {
                    config: config.clone(),
                },
                Some(stopped),
            )
            .await
            .map_err(unavailable)?
            {
                Response::Started => {
                    // Old helpers keep the original start/next wire format.
                    // Only negotiate the extra operation after an explicit hint.
                    let encoding_budget = if provider.encoding_budget_supported {
                        match request(&provider, Operation::EncodingBudget { bits_per_second: 0 }, None)
                            .await.map_err(unavailable)? {
                            Response::EncodingBudget(budget) => budget,
                            _ => return Err(unavailable("invalid media encoding budget reply")),
                        }
                    } else { None };
                    *provider
                        .active_config
                        .lock()
                        .map_err(|_| unavailable("media provider state unavailable"))? =
                        Some(config);
                    Ok(Box::new(HelperSource {
                        capture_timings: None,
                        provider,
                        _stop: stop,
                        encoding_budget,
                    }) as Box<dyn DesktopSource>)
                }
                _ => Err(unavailable("invalid media start reply")),
            }
        })
    }
}
struct HelperSource {
    provider: Arc<Provider>,
    _stop: oneshot::Sender<()>,
    encoding_budget: Option<u32>,
    capture_timings: Option<pab_protocol::DesktopCaptureTimings>,
}
impl Drop for HelperSource {
    fn drop(&mut self) {
        // Mark unavailable synchronously before the next generation queries
        // screens. The actor then closes the pipe and awaits helper restart.
        self.provider.retired.store(true, Ordering::Release);
        self.provider.ended.send_replace(true);
        CHANGED.notify_waiters();
    }
}
impl DesktopSource for HelperSource {
    fn capture_timings(&self) -> Option<pab_protocol::DesktopCaptureTimings> { self.capture_timings.clone() }
    fn encoding_budget(&self) -> Option<u32> { self.encoding_budget }
    fn set_encoding_budget(&mut self, bits_per_second: u32) -> MediaFuture<'_, ()> {
        Box::pin(async move {
            if self.encoding_budget.is_none() {
                return Err(DesktopError::Rejected { code: pab_protocol::DesktopErrorCode::Unsupported,
                    message: "media helper encoding budgets are unavailable".into() });
            }
            match request(&self.provider, Operation::EncodingBudget { bits_per_second }, None)
                .await.map_err(unavailable)? {
                Response::EncodingBudget(budget) => { self.encoding_budget = budget; Ok(()) },
                _ => Err(unavailable("invalid media encoding budget reply")),
            }
        })
    }
    fn next_frame(&mut self) -> MediaFuture<'_, MediaFrame> {
        Box::pin(async {
            let operation = if self.provider.timings_supported { Operation::NextTimed } else { Operation::Next };
            match request(&self.provider, operation, None)
                .await
                .map_err(unavailable)?
            {
                Response::Frame(frame, timings) => { self.capture_timings = timings; Ok(frame) },
                _ => Err(unavailable("invalid media frame reply")),
            }
        })
    }
}

pub(super) async fn serve_registered(
    mut socket: WebSocketStream<TcpStream>,
    frame: Value,
) -> Result<(), LocalIpcError> {
    if frame["version"].as_u64() != Some(VERSION.into()) {
        return Err(LocalIpcError::Protocol);
    }
    // Same kernel PID/account challenge; SYSTEM is accepted only in an
    // interactive session for the supervisor's secure-desktop media helper.
    let identity = Arc::new(
        helper_identity::verify_media(&mut socket, &frame)
            .await?
            .ok_or(LocalIpcError::Authentication)?,
    );
    // Video control and compressed chunks are separate WebSocket messages.
    // Do not let TCP coalesce a small message behind a delayed loopback ACK.
    // This socket is dedicated to authenticated media, not the shared IPC.
    socket.get_ref().set_nodelay(true)?;
    let id = NEXT_PROVIDER_ID.fetch_add(1, Ordering::Relaxed);
    let (sender, mut requests) = mpsc::channel(1);
    let _registration = Registration(id);
    send_json(
        &mut socket,
        &json!({"type":"media_helper_registered","version":VERSION,"provider_id":id}),
    )
    .await?;
    providers()
        .lock()
        .map_err(|_| LocalIpcError::Protocol)?
        .push(Arc::new(Provider {
            id,
            // Routing preference only, after the existing authenticated peer
            // challenge. This hint grants no extra permission or capability.
            supervised: cfg!(target_os = "linux") && frame["supervised"].as_bool().unwrap_or(false),
            sender,
            retired: AtomicBool::new(false),
            identity: identity.clone(),
            active_config: Mutex::new(None),
            input_supported: frame["input_supported"].as_bool().unwrap_or(false),
            clipboard_supported: frame["clipboard_version"].as_u64() == Some(pab_protocol::DESKTOP_CLIPBOARD_VERSION.into()),
            input: Mutex::new(None),
            cursor_supported: frame["cursor_schema_version"].as_u64()
                == Some(pab_protocol::DESKTOP_CURSOR_SCHEMA_VERSION.into()),
            encoding_budget_supported: frame["encoding_budget_version"].as_u64()
                == Some(ENCODING_BUDGET_VERSION.into()),
            timings_supported: frame["capture_timings_version"].as_u64() == Some(1),
            cursor: Mutex::new(None),
            ended: tokio::sync::watch::channel(false).0,
        }));
    CHANGED.notify_waiters();
    let mut stop: Option<oneshot::Receiver<()>> = None;
    let retired = identity.wait_retired();
    tokio::pin!(retired);
    loop {
        let request = tokio::select! {
            request = requests.recv() => request.ok_or(LocalIpcError::Closed)?,
            _ = stopped(&mut stop) => return Ok(()),
            _ = receive_frame(&mut socket) => return Err(LocalIpcError::Closed),
            _ = &mut retired => return Err(LocalIpcError::Authentication),
        };
        if !identity.current() {
            return Err(LocalIpcError::Authentication);
        }
        let Request {
            operation,
            stop: new_stop,
            mut reply,
        } = request;
        if reply.is_closed() {
            continue;
        }
        if matches!(operation, Operation::Start { .. }) {
            if stop.is_some() {
                return Err(LocalIpcError::Protocol);
            }
            stop = new_stop;
        }
        let result = tokio::select! {
            result = exchange(&mut socket, &operation) => result,
            _ = reply.closed() => return Ok(()),
            _ = stopped(&mut stop) => return Ok(()),
            _ = &mut retired => return Err(LocalIpcError::Authentication),
        };
        let failed = result.is_err();
        if reply.send(result).is_err() || failed {
            return Ok(());
        }
    }
}
async fn stopped(stop: &mut Option<oneshot::Receiver<()>>) {
    match stop {
        Some(stop) => {
            let _ = stop.await;
        }
        None => std::future::pending().await,
    }
}
async fn exchange(
    socket: &mut WebSocketStream<TcpStream>,
    operation: &Operation,
) -> Result<Response, LocalIpcError> {
    send_json(socket, &serde_json::to_value(operation)?).await?;
    let reply: Reply = read_control(socket).await?;
    match (operation, reply) {
        (_, Reply::Error { message }) => {
            Err(LocalIpcError::Remote(message.chars().take(1024).collect()))
        }
        (Operation::Screens, Reply::Screens { screens }) => {
            validate_desktop_screens(&screens).map_err(|_| LocalIpcError::Protocol)?;
            Ok(Response::Screens(screens))
        }
        (Operation::Start { .. }, Reply::Started) => Ok(Response::Started),
        (Operation::EncodingBudget { bits_per_second: requested },
            Reply::EncodingBudget { bits_per_second })
            if bits_per_second.is_none_or(|value| value == *requested) =>
            Ok(Response::EncodingBudget(bits_per_second)),
        (
            Operation::Next | Operation::NextTimed,
            Reply::Frame {
                bytes: size,
                capture_time_us,
                keyframe,
                timings,
            },
        ) if size > 0 && size <= DESKTOP_MAX_FRAME_BYTES => {
            let mut bytes = Vec::with_capacity(size);
            while bytes.len() < size {
                match receive_frame(socket).await? {
                    Message::Binary(chunk)
                        if !chunk.is_empty()
                            && chunk.len() <= DESKTOP_VIDEO_CHUNK_BYTES.min(size - bytes.len()) =>
                    {
                        bytes.extend_from_slice(&chunk)
                    }
                    _ => return Err(LocalIpcError::Protocol),
                }
            }
            Ok(Response::Frame(MediaFrame {
                bytes,
                capture_time_us,
                keyframe,
            }, timings))
        }
        _ => Err(LocalIpcError::Protocol),
    }
}
async fn read_control<S: AsyncRead + AsyncWrite + Unpin, T: serde::de::DeserializeOwned>(
    socket: &mut WebSocketStream<S>,
) -> Result<T, LocalIpcError> {
    match receive_frame(socket).await? {
        Message::Text(text) if text.len() <= DESKTOP_CONTROL_BYTES => {
            Ok(serde_json::from_str(&text)?)
        }
        _ => Err(LocalIpcError::Protocol),
    }
}

/// Run on a separate connection in the active interactive helper. The caller
/// owns the desktop/session lifecycle and must drop this future on a switch.
pub async fn serve_helper(backend: &dyn DesktopBackend) -> Result<(), LocalIpcError> {
    serve_helper_socket(connect_local().await?, backend, true).await
}
/// Prefer the Linux system supervisor's media helper for new sessions, retaining
/// the GUI helper as a fallback when the system helper is unavailable.
#[cfg(target_os = "linux")]
pub async fn serve_supervised_helper(backend: &dyn DesktopBackend) -> Result<(), LocalIpcError> {
    serve_helper_socket_with_role(connect_local().await?, backend, true, true).await
}
async fn serve_helper_socket(
    socket: LocalSocket,
    backend: &dyn DesktopBackend,
    connect_auxiliary: bool,
) -> Result<(), LocalIpcError> {
    serve_helper_socket_with_role(socket, backend, connect_auxiliary, false).await
}
async fn serve_helper_socket_with_role(
    mut socket: LocalSocket,
    backend: &dyn DesktopBackend,
    connect_auxiliary: bool,
    supervised: bool,
) -> Result<(), LocalIpcError> {
    if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
        stream.set_nodelay(true)?;
    }
    send_json(
        &mut socket,
        &json!({"type":"register_media_helper","version":VERSION,"process_id":std::process::id(),"input_supported":backend.supports_input(),"supervised":supervised,
            "clipboard_version": backend.supports_clipboard().then_some(pab_protocol::DESKTOP_CLIPBOARD_VERSION),
            "encoding_budget_version": ENCODING_BUDGET_VERSION,
            "capture_timings_version": 1,
            "cursor_schema_version": backend.supports_cursor().then_some(pab_protocol::DESKTOP_CURSOR_SCHEMA_VERSION)}),
    )
    .await?;
    let provider_id = tokio::time::timeout(AUTH_TIMEOUT, async {
        loop {
            let frame: Value = read_control(&mut socket).await?;
            match frame["type"].as_str() {
                Some("verify_helper_identity") => helper_identity::answer(&frame).await?,
                Some("media_helper_registered")
                    if frame["version"].as_u64() == Some(VERSION.into()) =>
                {
                    return frame["provider_id"].as_u64().ok_or(LocalIpcError::Protocol);
                }
                Some("status") | Some("error") => {}
                _ => return Err(LocalIpcError::Protocol),
            }
        }
    })
    .await
    .map_err(|_| LocalIpcError::Timeout)??;
    tokio::select! {
        result = serve_media_requests(socket, backend) => result,
        result = async {
            if connect_auxiliary { input::serve_helper(backend, provider_id).await }
            else { std::future::pending().await }
        } => result,
        result = async {
            if connect_auxiliary { cursor::serve_helper(backend, provider_id).await }
            else { std::future::pending().await }
        } => result,
    }
}
async fn serve_media_requests(
    mut socket: LocalSocket,
    backend: &dyn DesktopBackend,
) -> Result<(), LocalIpcError> {
    let mut source: Option<Box<dyn DesktopSource>> = None;
    loop {
        let operation = read_control(&mut socket).await?;
        // Watch socket closure while capture waits on a static desktop. It must
        // not retain the GPU worker until the next desktop update.
        let work = async {
            match operation {
                Operation::Screens if source.is_none() => {
                    let screens = backend.screens().await.map_err(unavailable_local)?;
                    validate_desktop_screens(&screens).map_err(|_| LocalIpcError::Protocol)?;
                    Ok((Reply::Screens { screens }, None))
                }
                Operation::Start { config } if source.is_none() => {
                    config.validate().map_err(|_| LocalIpcError::Protocol)?;
                    source = Some(backend.start(config).await.map_err(unavailable_local)?);
                    Ok((Reply::Started, None))
                }
                Operation::Next | Operation::NextTimed => {
                    let frame = source
                        .as_mut()
                        .ok_or(LocalIpcError::Protocol)?
                        .next_frame()
                        .await
                        .map_err(unavailable_local)?;
                    if frame.bytes.is_empty() || frame.bytes.len() > DESKTOP_MAX_FRAME_BYTES {
                        return Err(LocalIpcError::Protocol);
                    }
                    Ok((
                        Reply::Frame {
                            bytes: frame.bytes.len(),
                            capture_time_us: frame.capture_time_us,
                            keyframe: frame.keyframe,
                            timings: if matches!(operation, Operation::NextTimed) {
                                source.as_ref().and_then(|source| source.capture_timings())
                            } else { None },
                        },
                        Some(frame.bytes),
                    ))
                }
                Operation::EncodingBudget { bits_per_second }
                    if bits_per_second == 0 || (150_000..=16_000_000).contains(&bits_per_second) => {
                    let source = source.as_mut().ok_or(LocalIpcError::Protocol)?;
                    if source.encoding_budget().is_some_and(|current| current != bits_per_second) {
                        source.set_encoding_budget(bits_per_second).await.map_err(unavailable_local)?;
                    }
                    Ok((Reply::EncodingBudget { bits_per_second: source.encoding_budget() }, None))
                }
                _ => Err(LocalIpcError::Protocol),
            }
        };
        let result = tokio::select! {
            result = work => result,
            _ = receive_frame(&mut socket) => return Err(LocalIpcError::Closed),
        };
        match result {
            Ok((reply, bytes)) => {
                send_json(&mut socket, &serde_json::to_value(reply)?).await?;
                if let Some(bytes) = bytes {
                    for chunk in bytes.chunks(DESKTOP_VIDEO_CHUNK_BYTES) {
                        socket.send(Message::Binary(chunk.to_vec().into())).await?;
                    }
                }
            }
            Err(error) => {
                send_json(
                    &mut socket,
                    &serde_json::to_value(Reply::Error {
                        message: error.to_string().chars().take(1024).collect(),
                    })?,
                )
                .await?;
                return Err(error);
            }
        }
    }
}
fn unavailable_local(error: DesktopError) -> LocalIpcError {
    LocalIpcError::Remote(error.to_string())
}

#[cfg(all(test, any(windows, target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    mod quality_tests {
        use super::*;
        include!("local_ipc_media_quality_tests.rs");
    }
    mod cursor_tests {
        use super::*;
        include!("local_ipc_media_cursor_tests.rs");
    }
    use pab_protocol::{DesktopCodec, DesktopColor, DesktopRotation, RequestId};
    use std::sync::atomic::AtomicUsize;
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    const WAIT: Duration = Duration::from_secs(5);
    #[derive(Default)]
    struct Backend {
        active: Arc<AtomicUsize>,
        starting: bool,
        pending_frame: bool,
        input_enabled: bool,
        input_active: Arc<AtomicUsize>,
        held: Arc<AtomicBool>,
        clipboard_enabled: bool,
        clipboard_failed: Arc<AtomicBool>,
        clipboard_text: Arc<Mutex<Option<String>>>,
    }
    struct TestInput(Arc<AtomicUsize>, Arc<AtomicBool>);
    impl Drop for TestInput {
        fn drop(&mut self) {
            self.1.store(false, Ordering::SeqCst);
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    impl pab_remote_desktop::DesktopInputSink for TestInput {
        fn apply(&mut self, event: pab_protocol::RemoteDesktopInput) -> MediaFuture<'_, ()> {
            Box::pin(async move {
                match event {
                    pab_protocol::RemoteDesktopInput::Key { down, .. } => {
                        self.1.store(down, Ordering::SeqCst)
                    }
                    pab_protocol::RemoteDesktopInput::ReleaseAll => {
                        self.1.store(false, Ordering::SeqCst)
                    }
                    _ => {}
                }
                Ok(())
            })
        }
    }
    struct Source(Arc<AtomicUsize>, bool);
    impl Drop for Source {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    impl DesktopSource for Source {
        fn capture_timings(&self) -> Option<pab_protocol::DesktopCaptureTimings> {
            Some(pab_protocol::DesktopCaptureTimings { wait_us: 1200, encode_us: 3400 })
        }
        fn next_frame(&mut self) -> MediaFuture<'_, MediaFrame> {
            Box::pin(async {
                if self.1 {
                    std::future::pending::<()>().await;
                }
                Ok(MediaFrame {
                    bytes: vec![42; 200_013],
                    capture_time_us: 1,
                    keyframe: true,
                })
            })
        }
    }
    fn config() -> DesktopVideoConfig {
        DesktopVideoConfig {
            session_id: RequestId::new(),
            generation: 1,
            fps: 15,
            codec: DesktopCodec::H264AnnexB,
            color: DesktopColor::Bt709Limited,
            screen: DesktopScreen {
                id: "test".into(),
                name: "Test".into(),
                x: -1920,
                y: 0,
                desktop_width: 1920,
                desktop_height: 1080,
                pixel_width: 1920,
                pixel_height: 1080,
                rotation: DesktopRotation::Identity,
                scale_milli: 1000,
                primary: true,
            },
        }
    }
    impl DesktopBackend for Backend {
        fn supports_clipboard(&self) -> bool { self.clipboard_enabled }
        fn clipboard(&self, _: DesktopVideoConfig, operation: pab_protocol::DesktopClipboardOperation) -> MediaFuture<'_, pab_protocol::DesktopClipboardSnapshot> {
            Box::pin(async move {
                if self.clipboard_failed.load(Ordering::SeqCst) { return Err(unavailable("clipboard occupied")); }
                let mut text = self.clipboard_text.lock().unwrap();
                Ok(match operation {
                    pab_protocol::DesktopClipboardOperation::Read { after } => pab_protocol::DesktopClipboardSnapshot { revision: 1, changed: after != Some(1), text: if after != Some(1) { text.clone() } else { None } },
                    pab_protocol::DesktopClipboardOperation::Write { text: value } => { *text = Some(value); pab_protocol::DesktopClipboardSnapshot { revision: 1, changed: false, text: None } }
                })
            })
        }
        fn supports_input(&self) -> bool {
            self.input_enabled
        }
        fn input(
            &self,
            _: DesktopVideoConfig,
        ) -> MediaFuture<'_, Box<dyn pab_remote_desktop::DesktopInputSink>> {
            Box::pin(async move {
                self.input_active.fetch_add(1, Ordering::SeqCst);
                Ok(
                    Box::new(TestInput(self.input_active.clone(), self.held.clone()))
                        as Box<dyn pab_remote_desktop::DesktopInputSink>,
                )
            })
        }
        fn screens(&self) -> MediaFuture<'_, Vec<DesktopScreen>> {
            Box::pin(async { Ok(vec![config().screen]) })
        }
        fn start(&self, _: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopSource>> {
            Box::pin(async {
                self.active.fetch_add(1, Ordering::SeqCst);
                let source = Source(self.active.clone(), self.pending_frame);
                if self.starting {
                    std::future::pending::<()>().await;
                }
                Ok(Box::new(source) as Box<dyn DesktopSource>)
            })
        }
    }
    struct Pair {
        server: tokio::task::JoinHandle<()>,
        helper: tokio::task::JoinHandle<()>,
    }
    impl Drop for Pair {
        fn drop(&mut self) {
            self.server.abort();
            self.helper.abort();
        }
    }
    async fn pair(backend: Arc<dyn DesktopBackend>) -> Pair {
        pair_with_role(backend, false).await
    }
    async fn pair_with_role(backend: Arc<dyn DesktopBackend>, supervised: bool) -> Pair {
        pair_with_budget_hint(backend, supervised, true).await
    }
    async fn pair_with_budget_hint(backend: Arc<dyn DesktopBackend>, supervised: bool, budget_hint: bool) -> Pair {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async_with_config(stream, None).await.unwrap();
            let mut frame: Value = read_control(&mut socket).await.unwrap();
            if !budget_hint {
                frame.as_object_mut().unwrap().remove("encoding_budget_version");
                frame.as_object_mut().unwrap().remove("capture_timings_version");
            }
            // Exercise the real kernel PID/account challenge. HMAC handshake
            // is independently covered by existing local_ipc tests.
            let _ = serve_registered(socket, frame).await;
        });
        let helper = tokio::spawn(async move {
            let (socket, _) = connect_async(format!("ws://{address}")).await.unwrap();
            let _ = serve_helper_socket_with_role(socket, backend.as_ref(), false, supervised).await;
        });
        Pair { server, helper }
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires an active local graphical user on the Linux VM"]
    async fn supervised_media_wins_over_later_gui_and_falls_back_after_retirement() {
        let _lock = LOCK.lock().await;
        assert!(providers().lock().unwrap().is_empty());
        let supervised = Arc::new(Backend::default());
        let system_pair = pair_with_role(supervised.clone(), true).await;
        HelperMediaBackend.screens().await.unwrap();
        let selected = provider().await.unwrap().id;
        assert!(provider().await.unwrap().supervised);
        let gui = Arc::new(Backend::default());
        let gui_pair = pair(gui.clone()).await;
        tokio::time::timeout(WAIT, async {
            while providers().lock().unwrap().len() != 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        assert_eq!(provider().await.unwrap().id, selected);
        let mut source = HelperMediaBackend.start(config()).await.unwrap();
        assert_eq!(source.next_frame().await.unwrap().bytes.len(), 200_013);
        count(&supervised, 1).await;
        count(&gui, 0).await;
        drop(source);
        tokio::time::timeout(WAIT, async {
            while !system_pair.server.is_finished() || !system_pair.helper.is_finished() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        count(&supervised, 0).await;
        assert!(!provider().await.unwrap().supervised);
        let mut fallback = HelperMediaBackend.start(config()).await.unwrap();
        assert_eq!(fallback.next_frame().await.unwrap().bytes.len(), 200_013);
        count(&gui, 1).await;
        drop(fallback);
        ended(gui_pair, &gui).await;
    }
    async fn count(backend: &Backend, expected: usize) {
        tokio::time::timeout(WAIT, async {
            while backend.active.load(Ordering::SeqCst) != expected {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn ended(pair: Pair, backend: &Backend) {
        tokio::time::timeout(WAIT, async {
            while !pair.server.is_finished() || !pair.helper.is_finished() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        count(backend, 0).await;
        assert!(providers().lock().unwrap().is_empty());
    }
    #[tokio::test]
    #[cfg_attr(target_os = "linux", ignore = "requires an active local graphical user; run on the Linux VM with --ignored")]
    async fn verified_helper_transfers_bounded_binary_and_retires_on_idle_drop() {
        let _lock = LOCK.lock().await;
        for _ in 0..2 {
            let backend = Arc::new(Backend::default());
            let pair = pair(backend.clone()).await;
            assert_eq!(HelperMediaBackend.screens().await.unwrap()[0].x, -1920);
            let mut source = HelperMediaBackend.start(config()).await.unwrap();
            let frame = source.next_frame().await.unwrap();
            assert_eq!(frame.bytes, vec![42; 200_013]);
            count(&backend, 1).await;
            drop(source);
            ended(pair, &backend).await;
        }
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "requires an isolated profile and coordinated signed Aqua session helper on the authorized Apple host"]
    async fn macos_launchagent_native_media_reopens_and_retires() {
        use pab_desktop_media::software::SoftwareDecoder;
        let _lock = LOCK.lock().await;
        let root = pab_agent_core::DataPaths::for_scope(pab_agent_core::DataScope::Machine)
            .unwrap().root().canonicalize().unwrap();
        assert!(root.starts_with("/private/tmp"));
        assert!(root.file_name().unwrap().to_str().unwrap().starts_with("pab-helper-ipc-"));
        assert!(super::super::local_port().unwrap() > 1024);
        assert!(providers().lock().unwrap().is_empty());
        // The coordinator supplies the same private profile to the real signed
        // App through a fresh Aqua LaunchAgent. No installed service/token is used.
        super::super::issue_local_access(&root.join(super::super::TOKEN_FILE)).unwrap();
        struct Server(tokio::task::JoinHandle<()>);
        impl Drop for Server { fn drop(&mut self) { self.0.abort(); } }
        let _server = Server(super::super::spawn_local_service(root.clone()));
        std::fs::write(root.join("server-ready"), b"ready").unwrap();
        tokio::time::timeout(Duration::from_secs(30), async {
            while providers().lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.unwrap();
        for cycle in 0..2 {
            let screens = HelperMediaBackend.screens().await.unwrap();
            let mut config = config();
            config.screen = screens.into_iter().find(|screen| screen.primary).unwrap();
            let width = config.screen.pixel_width as usize;
            let height = config.screen.pixel_height as usize;
            let mut decoder = SoftwareDecoder::new(width, height).unwrap();
            let mut source = HelperMediaBackend.start(config.clone()).await.unwrap();
            let mut decoded = 0;
            for index in 0..30 {
                let frame = tokio::time::timeout(WAIT, source.next_frame()).await.unwrap().unwrap();
                if index == 0 { assert!(frame.keyframe); }
                if let Some(pixels) = decoder.decode(&frame.bytes).unwrap() {
                    assert_eq!(pixels.len(), width * height * 3 / 2);
                    decoded += 1;
                }
            }
            assert_eq!(decoded, 30);
            let id = provider().await.unwrap().id;
            let selected_screen = config.screen.id.clone();
            let mut input = HelperMediaBackend.input(config).await.unwrap();
            input.apply(pab_protocol::RemoteDesktopInput::ReleaseAll).await.unwrap();
            let fixture = root.join("input-fixture.json");
            if fixture.exists() {
                use pab_protocol::{DesktopMouseButton, RemoteDesktopInput as Event};
                let read = || serde_json::from_slice::<serde_json::Value>(&std::fs::read(&fixture).unwrap()).unwrap();
                let state = read();
                assert_eq!(state["focused"], true, "owned input fixture lost focus");
                assert_eq!(state["screen_id"], selected_screen);
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
                assert!((now - state["timestamp"].as_f64().unwrap()).abs() < 1.0, "stale fixture report");
                let x = u16::try_from(state["x"].as_u64().unwrap()).unwrap();
                let y = u16::try_from(state["y"].as_u64().unwrap()).unwrap();
                for event in [
                    Event::Move { x, y },
                    Event::Button { x, y, button: DesktopMouseButton::Left, down: true },
                    Event::Button { x, y, button: DesktopMouseButton::Left, down: false },
                    Event::Key { virtual_key: 0x41, down: true },
                    Event::Key { virtual_key: 0x41, down: false },
                    Event::Wheel { delta: 120 },
                    Event::Key { virtual_key: 0x10, down: true },
                ] { input.apply(event).await.unwrap(); }
                tokio::time::timeout(WAIT, async {
                    while read()["shifted"] != true { tokio::time::sleep(Duration::from_millis(10)).await; }
                }).await.unwrap();
                if cycle == 0 { input.apply(Event::ReleaseAll).await.unwrap(); }
            }
            drop(input);
            if fixture.exists() {
                tokio::time::timeout(WAIT, async {
                    loop {
                        let state: serde_json::Value = serde_json::from_slice(&std::fs::read(&fixture).unwrap()).unwrap();
                        if state["shift_up"].as_u64() == Some(cycle + 1) {
                            for key in ["downs", "ups", "keys_down", "keys_up", "wheels", "shift_down"] {
                                assert_eq!(state[key].as_u64(), Some(cycle + 1), "unexpected fixture count: {key}");
                            }
                            assert_eq!(state["last_key"], 0); // macOS virtual code for A.
                            assert_eq!(state["shifted"], false);
                            assert!((state["click_x"].as_f64().unwrap() - 100.0).abs() <= 2.0);
                            assert!((state["click_y"].as_f64().unwrap() - 100.0).abs() <= 2.0);
                            println!("Mac owned fixture cycle {cycle}: exact click/key/wheel counts and coordinates, Shift released via {}", if cycle == 0 { "ReleaseAll" } else { "input close" });
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }).await.unwrap();
            }
            drop(source);
            tokio::time::timeout(WAIT, async {
                while providers().lock().unwrap().iter().any(|p| p.id == id) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.unwrap();
            println!("Mac Aqua helper cycle {cycle}: {width}x{height}, {decoded} decoded frames, paired native release acknowledged, media registration retired");
        }
        std::fs::write(root.join("cycles-complete"), b"ready").unwrap();
        tokio::time::timeout(Duration::from_secs(30), async {
            while !root.join("helper-stopped").exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            while !providers().lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.unwrap();
        println!("Mac Aqua helper stopped; all media registrations retired");
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires active GNOME and sibling pab-linux-media-worker-cargo; run on the Linux VM"]
    async fn linux_native_capture_crosses_verified_helper_pipe_and_reopens() {
        use pab_desktop_media::{NativeBackend, software::SoftwareDecoder};
        let _lock = LOCK.lock().await;
        let worker = std::env::current_exe().unwrap().parent().unwrap().join("pab-linux-media-worker-cargo");
        assert!(worker.is_file(), "place the Cargo-built media worker beside this test binary");
        let backend = Arc::new(NativeBackend::new(worker, vec![]).with_input());
        assert!(HelperMediaBackend.supports_input() && backend.supports_input());
        for cycle in 0..2 {
            let pair = pair(backend.clone()).await;
            let screens = HelperMediaBackend.screens().await.unwrap();
            let mut config = config();
            config.screen = screens.into_iter().find(|screen| screen.primary).unwrap();
            let width = config.screen.pixel_width as usize;
            let height = config.screen.pixel_height as usize;
            let mut decoder = SoftwareDecoder::new(width, height).unwrap();
            let mut source = HelperMediaBackend.start(config.clone()).await.unwrap();
            let frame = tokio::time::timeout(WAIT, source.next_frame()).await.unwrap().unwrap();
            assert!(frame.keyframe);
            assert_eq!(decoder.decode(&frame.bytes).unwrap().unwrap().len(), width * height * 3 / 2);
            let id = providers().lock().unwrap()[0].id;
            let input_pair = input_pair(backend.clone(), id).await;
            let mut sink = HelperMediaBackend.input(config).await.unwrap();
            // Exercise the real native input worker without typing into an
            // arbitrary user window. Actual input has its owned GTK fixture.
            sink.apply(pab_protocol::RemoteDesktopInput::ReleaseAll).await.unwrap();
            drop(sink);
            drop(source);
            tokio::time::timeout(WAIT, async {
                while !pair.server.is_finished() || !pair.helper.is_finished()
                    || !input_pair.server.is_finished() || !input_pair.helper.is_finished() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                backend.idle().await;
            }).await.unwrap();
            assert!(providers().lock().unwrap().is_empty());
            println!("Linux native helper cycle {cycle}: {width}x{height}, decoded, paired native input acknowledged, workers reaped, registration retired");
        }
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires isolated root systemd unit, sibling Desktop and coordinated GDM switch on the authorized Linux VM"]
    async fn linux_supervisor_authenticated_media_retires_across_console_switch() {
        use pab_desktop_media::software::SoftwareDecoder;
        use pab_os_control::execution::{LinuxGraphicalSession, process_user_identity};
        use pab_protocol::RemoteDesktopInput;
        let _lock = LOCK.lock().await;
        assert_eq!(process_user_identity(std::process::id()).unwrap().account_id, "uid:0");
        let root = pab_agent_core::DataPaths::for_scope(pab_agent_core::DataScope::Machine).unwrap().root().to_path_buf();
        assert_eq!(root.parent(), Some(std::path::Path::new("/run")));
        assert!(root.file_name().unwrap().to_str().unwrap().starts_with("pab-rd-supervisor-ipc-"));
        assert!(super::super::local_port().unwrap() > 1024);
        assert_eq!(LinuxGraphicalSession::active_console().unwrap().uid(), 1000);
        struct Server(tokio::task::JoinHandle<()>);
        impl Drop for Server { fn drop(&mut self) { self.0.abort(); } }
        let _server = Server(super::super::spawn_local_service(root.clone()));
        let executable = std::env::current_exe().unwrap().parent().unwrap().join("pab-desktop");
        let mut supervisor = tokio::process::Command::new(executable).arg("--media-supervisor")
            .kill_on_drop(true).spawn().unwrap();
        let phase = |name: &str| {
            std::fs::write(root.join("phase.tmp"), name).unwrap();
            std::fs::rename(root.join("phase.tmp"), root.join("phase")).unwrap();
        };
        async fn switched(uid: u32) {
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    if LinuxGraphicalSession::active_console().is_ok_and(|s| s.uid() == uid) { break; }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }).await.unwrap();
        }
        let mut source = None;
        let mut input: Option<Box<dyn pab_remote_desktop::DesktopInputSink>> = None;
        for (index, uid, prefix) in [(0, 1000, "gnome:"), (1, 128, "x11:"), (2, 1000, "gnome:")] {
            switched(uid).await;
            if index == 1 {
                // The external coordinator checks the real GDM session manager,
                // not just logind's early active-seat notification. Xorg output
                // geometry and the compositor can still change during startup.
                tokio::time::timeout(Duration::from_secs(25), async {
                    while !root.join("greeter-running").exists() {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }).await.unwrap();
            }
            if let Some(mut old) = source.take() {
                // An old provider cannot continue serving the new active seat.
                let old: &mut Box<dyn DesktopSource> = &mut old;
                assert!(tokio::time::timeout(WAIT, old.next_frame()).await.unwrap().is_err());
            }
            if let Some(mut old) = input.take() {
                assert!(tokio::time::timeout(WAIT, old.apply(RemoteDesktopInput::ReleaseAll)).await.unwrap().is_err());
            }
            let screens = tokio::time::timeout(Duration::from_secs(25), async {
                loop {
                    if let Ok(screens) = HelperMediaBackend.screens().await {
                        if screens.iter().any(|s| s.id.starts_with(prefix)) { break screens; }
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }).await.unwrap();
            let mut selected = config();
            selected.screen = screens.into_iter().find(|s| s.primary).unwrap();
            let width = selected.screen.pixel_width as usize;
            let height = selected.screen.pixel_height as usize;
            let mut current = HelperMediaBackend.start(selected.clone()).await.unwrap();
            let frame = tokio::time::timeout(WAIT, current.next_frame()).await.unwrap().unwrap();
            let mut decoder = SoftwareDecoder::new(width, height).unwrap();
            assert!(frame.keyframe);
            assert_eq!(decoder.decode(&frame.bytes).unwrap().unwrap().len(), width * height * 3 / 2);
            let mut sink = HelperMediaBackend.input(selected).await.unwrap();
            sink.apply(RemoteDesktopInput::ReleaseAll).await.unwrap();
            if index == 1 {
                // The coordinator independently observes this test-owned Shift
                // and checks its release while leaving the greeter.
                sink.apply(RemoteDesktopInput::Key { virtual_key: 0xa0, down: true }).await.unwrap();
            }
            println!("supervised authenticated phase {index}: uid={uid}, {width}x{height}, decoded, native input acknowledged");
            source = Some(current); input = Some(sink);
            phase(match index { 0 => "user-ready", 1 => "greeter-ready", _ => "returned-user" });
        }
        input.as_mut().unwrap().apply(RemoteDesktopInput::ReleaseAll).await.unwrap();
        drop(input); drop(source);
        let status = tokio::process::Command::new("/bin/kill")
            .args(["-TERM", &supervisor.id().unwrap().to_string()]).status().await.unwrap();
        assert!(status.success());
        assert!(tokio::time::timeout(Duration::from_secs(10), supervisor.wait()).await.unwrap().unwrap().success());
        tokio::time::timeout(WAIT, async {
            while !providers().lock().unwrap().is_empty() { tokio::time::sleep(Duration::from_millis(20)).await; }
        }).await.unwrap();
        phase("complete");
    }
    #[tokio::test]
    #[cfg_attr(target_os = "linux", ignore = "requires an active local graphical user; run on the Linux VM with --ignored")]
    async fn cancellation_during_start_or_static_capture_releases_helper() {
        let _lock = LOCK.lock().await;
        for starting in [true, false] {
            let backend = Arc::new(Backend {
                starting,
                pending_frame: true,
                ..Default::default()
            });
            let pair = pair(backend.clone()).await;
            HelperMediaBackend.screens().await.unwrap();
            if starting {
                assert!(
                    tokio::time::timeout(
                        Duration::from_millis(200),
                        HelperMediaBackend.start(config())
                    )
                    .await
                    .is_err()
                );
            } else {
                let mut source = HelperMediaBackend.start(config()).await.unwrap();
                assert!(
                    tokio::time::timeout(Duration::from_millis(200), source.next_frame())
                        .await
                        .is_err()
                );
                drop(source);
            }
            ended(pair, &backend).await;
        }
    }

    async fn input_pair(backend: Arc<dyn DesktopBackend>, provider_id: u64) -> Pair {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async_with_config(stream, None).await.unwrap();
            let frame: Value = read_control(&mut socket).await.unwrap();
            let _ = input::serve_registered(socket, frame).await;
        });
        let helper = tokio::spawn(async move {
            let (socket, _) = connect_async(format!("ws://{address}")).await.unwrap();
            let _ = input::serve_helper_socket(socket, backend.as_ref(), provider_id).await;
        });
        Pair { server, helper }
    }

    #[tokio::test]
    #[cfg_attr(target_os = "linux", ignore = "requires an active local graphical user")]
    async fn paired_clipboard_failure_preserves_input_and_static_capture() {
        let _lock = LOCK.lock().await;
        let backend = Arc::new(Backend { pending_frame: true, input_enabled: true, clipboard_enabled: true, ..Default::default() });
        let capture = pair(backend.clone()).await;
        let config = config();
        let source = HelperMediaBackend.start(config.clone()).await.unwrap();
        let id = providers().lock().unwrap()[0].id;
        let input_channel = input_pair(backend.clone(), id).await;
        assert!(HelperMediaBackend.clipboard(config.clone(), pab_protocol::DesktopClipboardOperation::Read { after: None }).await.is_err());
        let mut input = HelperMediaBackend.input(config.clone()).await.unwrap();
        let text = "中文\nclipboard 🖥️".repeat(5000);
        HelperMediaBackend.clipboard(config.clone(), pab_protocol::DesktopClipboardOperation::Write { text: text.clone() }).await.unwrap();
        let read = HelperMediaBackend.clipboard(config.clone(), pab_protocol::DesktopClipboardOperation::Read { after: None }).await.unwrap();
        assert_eq!(read.text.as_ref(), Some(&text));
        backend.clipboard_failed.store(true, Ordering::SeqCst);
        assert!(HelperMediaBackend.clipboard(config.clone(), pab_protocol::DesktopClipboardOperation::Read { after: None }).await.is_err());
        input.apply(pab_protocol::RemoteDesktopInput::Key { virtual_key: 65, down: true }).await.unwrap();
        assert!(backend.held.load(Ordering::SeqCst));
        assert_eq!(backend.input_active.load(Ordering::SeqCst), 1);
        backend.clipboard_failed.store(false, Ordering::SeqCst);
        assert!(HelperMediaBackend.clipboard(config, pab_protocol::DesktopClipboardOperation::Read { after: None }).await.is_ok());
        input.apply(pab_protocol::RemoteDesktopInput::ReleaseAll).await.unwrap();
        assert!(!backend.held.load(Ordering::SeqCst));
        drop(input); drop(source); drop(input_channel); drop(capture);
    }

    #[tokio::test]
    #[cfg_attr(target_os = "linux", ignore = "requires an active local graphical user; run on the Linux VM with --ignored")]
    async fn paired_input_works_during_static_capture_and_releases_on_socket_close() {
        let _lock = LOCK.lock().await;
        for close_input in [true, false] {
            let backend = Arc::new(Backend {
                pending_frame: true,
                input_enabled: true,
                ..Default::default()
            });
            let pair = pair(backend.clone()).await;
            let config = config();
            let mut source = HelperMediaBackend.start(config.clone()).await.unwrap();
            let id = providers().lock().unwrap()[0].id;
            let wrong = input_pair(backend.clone(), u64::MAX).await;
            tokio::time::timeout(WAIT, async {
                while !wrong.server.is_finished() || !wrong.helper.is_finished() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            drop(wrong);
            assert!(
                providers().lock().unwrap()[0]
                    .input
                    .lock()
                    .unwrap()
                    .is_none()
            );
            let input_pair = input_pair(backend.clone(), id).await;
            let mut sink = Some(HelperMediaBackend.input(config.clone()).await.unwrap());
            let mut stale = config.clone();
            stale.generation += 1;
            assert!(HelperMediaBackend.input(stale).await.is_err());
            {
                let pending = source.next_frame();
                tokio::pin!(pending);
                tokio::select! {
                    _ = &mut pending => panic!("static source unexpectedly returned"),
                    _ = tokio::time::sleep(Duration::from_millis(50)) => {},
                }
                sink.as_mut()
                    .unwrap()
                    .apply(pab_protocol::RemoteDesktopInput::Key {
                        virtual_key: 65,
                        down: true,
                    })
                    .await
                    .unwrap();
                assert!(backend.held.load(Ordering::SeqCst));
                if close_input {
                    sink = None;
                    tokio::time::timeout(WAIT, async {
                        while backend.input_active.load(Ordering::SeqCst) != 0 {
                            tokio::task::yield_now().await;
                        }
                    })
                    .await
                    .unwrap();
                    assert!(!backend.held.load(Ordering::SeqCst));
                    assert_eq!(backend.active.load(Ordering::SeqCst), 1);
                }
                // Ending the pending request retires capture. In the second
                // case its input lease is still alive and must become unusable.
            }
            drop(source);
            tokio::time::timeout(WAIT, async {
                while backend.input_active.load(Ordering::SeqCst) != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(!backend.held.load(Ordering::SeqCst));
            if let Some(mut sink) = sink {
                assert!(
                    sink.apply(pab_protocol::RemoteDesktopInput::ReleaseAll)
                        .await
                        .is_err()
                );
            }
            drop(input_pair);
            ended(pair, &backend).await;
        }
    }
}
