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

const VERSION: u16 = 1;
static PROVIDERS: OnceLock<Mutex<Vec<Arc<Provider>>>> = OnceLock::new();
static CHANGED: Notify = Notify::const_new();
fn providers() -> &'static Mutex<Vec<Arc<Provider>>> {
    PROVIDERS.get_or_init(|| Mutex::new(Vec::new()))
}
struct Provider {
    id: u64,
    sender: mpsc::Sender<Request>,
    retired: AtomicBool,
    identity: Arc<helper_identity::VerifiedMediaHelper>,
    active_config: Mutex<Option<DesktopVideoConfig>>,
    input_supported: bool,
    input: Mutex<Option<Arc<input::InputProvider>>>,
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
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    Screens {
        screens: Vec<DesktopScreen>,
    },
    Started,
    Frame {
        bytes: usize,
        capture_time_us: u64,
        keyframe: bool,
    },
    Error {
        message: String,
    },
}
enum Response {
    Screens(Vec<DesktopScreen>),
    Started,
    Frame(MediaFrame),
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
                .rev()
                .find(|p| !p.sender.is_closed() && !p.retired.load(Ordering::Acquire))
                .cloned()
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

/// Supported implementation is advertised independently of transient helper
/// availability. Failure here is a media error, not a device connection error.
pub struct HelperMediaBackend;
impl DesktopBackend for HelperMediaBackend {
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
                    *provider
                        .active_config
                        .lock()
                        .map_err(|_| unavailable("media provider state unavailable"))? =
                        Some(config);
                    Ok(Box::new(HelperSource {
                        provider,
                        _stop: stop,
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
    fn next_frame(&mut self) -> MediaFuture<'_, MediaFrame> {
        Box::pin(async {
            match request(&self.provider, Operation::Next, None)
                .await
                .map_err(unavailable)?
            {
                Response::Frame(frame) => Ok(frame),
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
            sender,
            retired: AtomicBool::new(false),
            identity: identity.clone(),
            active_config: Mutex::new(None),
            input_supported: frame["input_supported"].as_bool().unwrap_or(false),
            input: Mutex::new(None),
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
        (
            Operation::Next,
            Reply::Frame {
                bytes: size,
                capture_time_us,
                keyframe,
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
            }))
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
async fn serve_helper_socket(
    mut socket: LocalSocket,
    backend: &dyn DesktopBackend,
    connect_input: bool,
) -> Result<(), LocalIpcError> {
    send_json(
        &mut socket,
        &json!({"type":"register_media_helper","version":VERSION,"process_id":std::process::id(),"input_supported":backend.supports_input()}),
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
            if connect_input { input::serve_helper(backend, provider_id).await }
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
                Operation::Next => {
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
                        },
                        Some(frame.bytes),
                    ))
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
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async_with_config(stream, None).await.unwrap();
            let frame: Value = read_control(&mut socket).await.unwrap();
            // Exercise the real kernel PID/account challenge. HMAC handshake
            // is independently covered by existing local_ipc tests.
            let _ = serve_registered(socket, frame).await;
        });
        let helper = tokio::spawn(async move {
            let (socket, _) = connect_async(format!("ws://{address}")).await.unwrap();
            let _ = serve_helper_socket(socket, backend.as_ref(), false).await;
        });
        Pair { server, helper }
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
