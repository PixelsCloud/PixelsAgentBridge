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
}
struct Registration(u64);
impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut providers) = providers().lock() {
            providers.retain(|p| p.id != self.0);
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
            match request(&provider, Operation::Start { config }, Some(stopped))
                .await
                .map_err(unavailable)?
            {
                Response::Started => Ok(Box::new(HelperSource {
                    provider,
                    _stop: stop,
                }) as Box<dyn DesktopSource>),
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
    let identity = helper_identity::verify_media(&mut socket, &frame)
        .await?
        .ok_or(LocalIpcError::Authentication)?;
    let id = NEXT_PROVIDER_ID.fetch_add(1, Ordering::Relaxed);
    let (sender, mut requests) = mpsc::channel(1);
    let _registration = Registration(id);
    send_json(
        &mut socket,
        &json!({"type":"media_helper_registered","version":VERSION}),
    )
    .await?;
    providers()
        .lock()
        .map_err(|_| LocalIpcError::Protocol)?
        .push(Arc::new(Provider {
            id,
            sender,
            retired: AtomicBool::new(false),
        }));
    CHANGED.notify_waiters();
    let mut stop: Option<oneshot::Receiver<()>> = None;
    loop {
        let request = tokio::select! {
            request = requests.recv() => request.ok_or(LocalIpcError::Closed)?,
            _ = stopped(&mut stop) => return Ok(()),
            _ = receive_frame(&mut socket) => return Err(LocalIpcError::Closed),
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
    serve_helper_socket(connect_local().await?, backend).await
}
async fn serve_helper_socket(
    mut socket: LocalSocket,
    backend: &dyn DesktopBackend,
) -> Result<(), LocalIpcError> {
    send_json(
        &mut socket,
        &json!({"type":"register_media_helper","version":VERSION,"process_id":std::process::id()}),
    )
    .await?;
    tokio::time::timeout(AUTH_TIMEOUT, async {
        loop {
            let frame: Value = read_control(&mut socket).await?;
            match frame["type"].as_str() {
                Some("verify_helper_identity") => helper_identity::answer(&frame).await?,
                Some("media_helper_registered")
                    if frame["version"].as_u64() == Some(VERSION.into()) =>
                {
                    return Ok(());
                }
                Some("status") | Some("error") => {}
                _ => return Err(LocalIpcError::Protocol),
            }
        }
    })
    .await
    .map_err(|_| LocalIpcError::Timeout)??;
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

#[cfg(all(test, windows))]
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
    async fn pair(backend: Arc<Backend>) -> Pair {
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
            let _ = serve_helper_socket(socket, backend.as_ref()).await;
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
    #[tokio::test]
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
}
