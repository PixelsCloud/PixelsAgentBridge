//! Pull-based cursor snapshots on the existing authenticated helper transport.
//! Paired to the capture's kernel-verified process and exact active generation.
//! No cursor request shares the video or legacy window request queue.
use super::*;
use pab_protocol::{DESKTOP_CURSOR_SCHEMA_VERSION, DesktopCursorHeader};
use pab_remote_desktop::{CursorUpdate, DesktopCursorSource};

#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum CursorOperation {
    Start { config: DesktopVideoConfig },
    Next,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
enum CursorReply {
    Started,
    Update { header: DesktopCursorHeader },
    Failed,
}
enum CursorResponse {
    Started,
    Update(CursorUpdate),
}
struct CursorRequest {
    operation: CursorOperation,
    stop: Option<oneshot::Receiver<()>>,
    reply: oneshot::Sender<Result<CursorResponse, LocalIpcError>>,
}
pub(super) struct CursorProvider {
    sender: mpsc::Sender<CursorRequest>,
    claimed: AtomicBool,
}
struct Registration {
    capture: Arc<Provider>,
    cursor: Arc<CursorProvider>,
}
impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut current) = self.capture.cursor.lock() {
            if current
                .as_ref()
                .is_some_and(|p| Arc::ptr_eq(p, &self.cursor))
            {
                *current = None;
            }
        }
        CHANGED.notify_waiters();
    }
}
struct CursorSource {
    capture: Arc<Provider>,
    cursor: Arc<CursorProvider>,
    _stop: oneshot::Sender<()>,
}
impl DesktopCursorSource for CursorSource {
    fn next_update(&mut self) -> MediaFuture<'_, CursorUpdate> {
        Box::pin(async {
            if self.capture.retired.load(Ordering::Acquire) {
                return Err(DesktopError::Closed);
            }
            match request(&self.cursor, CursorOperation::Next, None)
                .await
                .map_err(unavailable)?
            {
                CursorResponse::Update(update) => Ok(update),
                _ => Err(unavailable("invalid cursor reply")),
            }
        })
    }
}
async fn request(
    cursor: &CursorProvider,
    operation: CursorOperation,
    stop: Option<oneshot::Receiver<()>>,
) -> Result<CursorResponse, LocalIpcError> {
    let (reply, result) = oneshot::channel();
    cursor
        .sender
        .send(CursorRequest {
            operation,
            stop,
            reply,
        })
        .await
        .map_err(|_| LocalIpcError::Closed)?;
    result.await.map_err(|_| LocalIpcError::Closed)?
}
pub(super) async fn open(
    config: DesktopVideoConfig,
) -> Result<Box<dyn DesktopCursorSource>, DesktopError> {
    config.validate().map_err(unavailable)?;
    let capture = providers()
        .lock()
        .map_err(|_| unavailable("media provider unavailable"))?
        .iter()
        .find(|p| {
            !p.retired.load(Ordering::Acquire)
                && p.active_config
                    .lock()
                    .ok()
                    .is_some_and(|active| active.as_ref() == Some(&config))
        })
        .cloned()
        .ok_or_else(|| unavailable("capture generation ended"))?;
    if !capture.cursor_supported {
        return Err(unavailable("capture helper has no independent cursor"));
    }
    let cursor = tokio::time::timeout(AUTH_TIMEOUT, async {
        loop {
            let changed = CHANGED.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if capture.retired.load(Ordering::Acquire) {
                return Err(LocalIpcError::Closed);
            }
            if let Some(cursor) = capture
                .cursor
                .lock()
                .map_err(|_| LocalIpcError::Protocol)?
                .as_ref()
                .filter(|p| !p.sender.is_closed() && !p.claimed.load(Ordering::Acquire))
                .cloned()
            {
                if !cursor.claimed.swap(true, Ordering::AcqRel) {
                    return Ok(cursor);
                }
            }
            changed.await;
        }
    })
    .await
    .map_err(|_| DesktopError::Timeout)?
    .map_err(unavailable)?;
    let (stop, stopped) = oneshot::channel();
    let source = CursorSource {
        capture,
        cursor,
        _stop: stop,
    };
    match tokio::time::timeout(
        AUTH_TIMEOUT,
        request(
            &source.cursor,
            CursorOperation::Start { config },
            Some(stopped),
        ),
    )
    .await
    .map_err(|_| DesktopError::Timeout)?
    .map_err(unavailable)?
    {
        CursorResponse::Started => Ok(Box::new(source)),
        _ => Err(unavailable("invalid cursor start reply")),
    }
}

struct CursorState {
    config: DesktopVideoConfig,
    sequence: u64,
    image_id: u64,
}
impl CursorState {
    fn new(config: DesktopVideoConfig) -> Result<Self, LocalIpcError> {
        config.validate().map_err(|_| LocalIpcError::Protocol)?;
        Ok(Self {
            config,
            sequence: 1,
            image_id: 0,
        })
    }
    fn size(&self, header: &DesktopCursorHeader) -> Result<usize, LocalIpcError> {
        self.sequence
            .checked_add(1)
            .ok_or(LocalIpcError::Protocol)?;
        header
            .validate(&self.config, self.sequence, self.image_id)
            .map_err(|_| LocalIpcError::Protocol)
    }
    fn commit(&mut self, header: &DesktopCursorHeader) {
        self.sequence += 1;
        self.image_id = header.image_id;
    }
}
async fn capture_ended(capture: &Provider) {
    let mut ended = capture.ended.subscribe();
    loop {
        if *ended.borrow_and_update() {
            return;
        }
        if ended.changed().await.is_err() {
            return;
        }
    }
}
pub(in crate::local_ipc) async fn serve_registered(
    mut socket: WebSocketStream<TcpStream>,
    frame: Value,
) -> Result<(), LocalIpcError> {
    if frame["version"].as_u64() != Some(DESKTOP_CURSOR_SCHEMA_VERSION.into()) {
        return Err(LocalIpcError::Protocol);
    }
    let id = frame["provider_id"]
        .as_u64()
        .ok_or(LocalIpcError::Protocol)?;
    let identity = helper_identity::verify_media(&mut socket, &frame)
        .await?
        .ok_or(LocalIpcError::Authentication)?;
    let capture = providers()
        .lock()
        .map_err(|_| LocalIpcError::Protocol)?
        .iter()
        .find(|p| {
            p.id == id
                && p.cursor_supported
                && !p.retired.load(Ordering::Acquire)
                && p.identity.same_process(&identity)
        })
        .cloned()
        .ok_or(LocalIpcError::Authentication)?;
    let (sender, mut requests) = mpsc::channel(1);
    let cursor = Arc::new(CursorProvider {
        sender,
        claimed: AtomicBool::new(false),
    });
    {
        let mut current = capture.cursor.lock().map_err(|_| LocalIpcError::Protocol)?;
        if current.is_some() {
            return Err(LocalIpcError::Protocol);
        }
        *current = Some(cursor.clone());
    }
    let _registration = Registration {
        capture: capture.clone(),
        cursor,
    };
    send_json(
        &mut socket,
        &json!({"type":"media_cursor_registered","version":DESKTOP_CURSOR_SCHEMA_VERSION}),
    )
    .await?;
    CHANGED.notify_waiters();
    let mut stop = None;
    let mut state: Option<CursorState> = None;
    let retired = identity.wait_retired();
    tokio::pin!(retired);
    loop {
        let request = tokio::select! {
            biased;
            _ = capture_ended(&capture) => return Ok(()),
            _ = &mut retired => return Err(LocalIpcError::Authentication),
            _ = stopped(&mut stop) => return Ok(()),
            _ = receive_frame(&mut socket) => return Err(LocalIpcError::Closed),
            request = requests.recv() => request.ok_or(LocalIpcError::Closed)?,
        };
        let CursorRequest {
            operation,
            stop: new_stop,
            mut reply,
        } = request;
        if reply.is_closed() {
            return Ok(());
        }
        if !identity.current() {
            return Err(LocalIpcError::Authentication);
        }
        if let CursorOperation::Start { config } = &operation {
            if stop.is_some() || new_stop.is_none() {
                return Err(LocalIpcError::Protocol);
            }
            state = Some(CursorState::new(config.clone())?);
            stop = new_stop;
        }
        let state = state.as_mut().ok_or(LocalIpcError::Protocol)?;
        if capture
            .active_config
            .lock()
            .map_err(|_| LocalIpcError::Protocol)?
            .as_ref()
            != Some(&state.config)
        {
            return Err(LocalIpcError::Protocol);
        }
        let result = tokio::select! {
            biased;
            _ = capture_ended(&capture) => return Ok(()),
            _ = &mut retired => return Err(LocalIpcError::Authentication),
            _ = stopped(&mut stop) => return Ok(()),
            _ = reply.closed() => return Ok(()),
            result = exchange(&mut socket, &operation, state) => result,
        };
        let failed = result.is_err();
        if reply.send(result).is_err() || failed {
            return Ok(());
        }
    }
}
async fn exchange(
    socket: &mut WebSocketStream<TcpStream>,
    operation: &CursorOperation,
    state: &mut CursorState,
) -> Result<CursorResponse, LocalIpcError> {
    send_json(socket, &serde_json::to_value(operation)?).await?;
    // Next may wait indefinitely for a real change; capture/lease/peer closure
    // is watched by the caller. Image bytes after the header have a deadline.
    match (operation, read_control::<_, CursorReply>(socket).await?) {
        (CursorOperation::Start { .. }, CursorReply::Started) => Ok(CursorResponse::Started),
        (CursorOperation::Next, CursorReply::Update { header }) => {
            let size = state.size(&header)?;
            let bytes = tokio::time::timeout(AUTH_TIMEOUT, async {
                let mut bytes = Vec::with_capacity(size);
                while bytes.len() < size {
                    match receive_frame(socket).await? {
                        Message::Binary(chunk)
                            if !chunk.is_empty()
                                && chunk.len()
                                    <= DESKTOP_VIDEO_CHUNK_BYTES.min(size - bytes.len()) =>
                        {
                            bytes.extend_from_slice(&chunk)
                        }
                        _ => return Err(LocalIpcError::Protocol),
                    }
                }
                Ok::<_, LocalIpcError>(bytes)
            })
            .await
            .map_err(|_| LocalIpcError::Timeout)??;
            state.commit(&header);
            Ok(CursorResponse::Update(CursorUpdate { header, bytes }))
        }
        _ => Err(LocalIpcError::Protocol),
    }
}
pub(super) async fn serve_helper(
    backend: &dyn DesktopBackend,
    provider_id: u64,
) -> Result<(), LocalIpcError> {
    if !backend.supports_cursor() {
        return std::future::pending().await;
    }
    loop {
        if let Ok(socket) = connect_local().await {
            // Cursor pixels and native errors are never logged.
            let _ = serve_helper_socket(socket, backend, provider_id).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
pub(super) async fn serve_helper_socket(
    mut socket: LocalSocket,
    backend: &dyn DesktopBackend,
    provider_id: u64,
) -> Result<(), LocalIpcError> {
    send_json(
        &mut socket,
        &json!({"type":"register_media_cursor_helper","version":DESKTOP_CURSOR_SCHEMA_VERSION,
        "provider_id":provider_id,"process_id":std::process::id()}),
    )
    .await?;
    tokio::time::timeout(AUTH_TIMEOUT, async {
        loop {
            let frame: Value = read_control(&mut socket).await?;
            match frame["type"].as_str() {
                Some("verify_helper_identity") => helper_identity::answer(&frame).await?,
                Some("media_cursor_registered")
                    if frame["version"].as_u64() == Some(DESKTOP_CURSOR_SCHEMA_VERSION.into()) =>
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
    let mut source: Option<Box<dyn DesktopCursorSource>> = None;
    let mut state: Option<CursorState> = None;
    loop {
        let operation: CursorOperation = read_control(&mut socket).await?;
        let work = async {
            match operation {
                CursorOperation::Start { config } if source.is_none() => {
                    state = Some(CursorState::new(config.clone())?);
                    source = Some(
                        backend
                            .cursor(config)
                            .await
                            .map_err(|error| {
                                tracing::warn!(%error, "native cursor source startup failed");
                                LocalIpcError::Protocol
                            })?,
                    );
                    Ok((CursorReply::Started, None))
                }
                CursorOperation::Next if source.is_some() => {
                    let update = source
                        .as_mut()
                        .unwrap()
                        .next_update()
                        .await
                        .map_err(|error| {
                            tracing::warn!(%error, "native cursor source update failed");
                            LocalIpcError::Protocol
                        })?;
                    let state = state.as_mut().ok_or(LocalIpcError::Protocol)?;
                    if state.size(&update.header)? != update.bytes.len() {
                        return Err(LocalIpcError::Protocol);
                    }
                    state.commit(&update.header);
                    Ok((
                        CursorReply::Update {
                            header: update.header,
                        },
                        Some(update.bytes),
                    ))
                }
                _ => Err(LocalIpcError::Protocol),
            }
        };
        let result = tokio::select! {
            result = work => result,
            _ = receive_frame(&mut socket) => return Err(LocalIpcError::Closed),
        };
        let (reply, bytes) = match result {
            Ok(value) => value,
            Err(error) => {
                send_json(&mut socket, &serde_json::to_value(CursorReply::Failed)?).await?;
                return Err(error);
            }
        };
        tokio::time::timeout(AUTH_TIMEOUT, async {
            send_json(&mut socket, &serde_json::to_value(reply)?).await?;
            if let Some(bytes) = bytes {
                for chunk in bytes.chunks(DESKTOP_VIDEO_CHUNK_BYTES) {
                    socket.send(Message::Binary(chunk.to_vec().into())).await?;
                }
            }
            Ok::<_, LocalIpcError>(())
        })
        .await
        .map_err(|_| LocalIpcError::Timeout)??;
    }
}
