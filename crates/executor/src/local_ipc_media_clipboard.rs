//! Clipboard payloads never share the keyboard/input request socket.
//! Registration requires the capture helper's kernel-verified process identity.
use super::*;
use pab_protocol::{
    DESKTOP_CLIPBOARD_CHUNK_BYTES, DESKTOP_CLIPBOARD_TIMEOUT_SECS, DESKTOP_CLIPBOARD_VERSION,
    DesktopClipboardImage, DesktopClipboardOperation, DesktopClipboardSnapshot,
};

struct Request {
    config: DesktopVideoConfig,
    operation: DesktopClipboardOperation,
    reply: oneshot::Sender<Result<DesktopClipboardSnapshot, LocalIpcError>>,
}
pub(super) struct ClipboardProvider {
    sender: mpsc::Sender<Request>,
}
struct Registration(Arc<Provider>, Arc<ClipboardProvider>);
impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.0.clipboard.lock() {
            if slot.as_ref().is_some_and(|p| Arc::ptr_eq(p, &self.1)) {
                *slot = None;
            }
        }
        CHANGED.notify_waiters();
    }
}
fn authorized(capture: &Provider, config: &DesktopVideoConfig) -> bool {
    !capture.retired.load(Ordering::Acquire)
        && capture.identity.current()
        && capture
            .active_config
            .lock()
            .is_ok_and(|c| c.as_ref() == Some(config))
        && capture
            .input
            .lock()
            .is_ok_and(|c| c.as_ref().is_some_and(|i| i.active()))
}
async fn revoked(capture: &Provider, config: &DesktopVideoConfig) {
    while authorized(capture, config) {
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}
pub(super) async fn exchange(
    config: DesktopVideoConfig,
    operation: DesktopClipboardOperation,
) -> Result<DesktopClipboardSnapshot, DesktopError> {
    operation.validate().map_err(unavailable)?;
    let capture = providers()
        .lock()
        .map_err(|_| unavailable("clipboard provider unavailable"))?
        .iter()
        .find(|p| p.clipboard_supported && authorized(p, &config))
        .cloned()
        .ok_or(DesktopError::Closed)?;
    let channel = tokio::time::timeout(AUTH_TIMEOUT, async {
        loop {
            let changed = CHANGED.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if !authorized(&capture, &config) {
                return Err(LocalIpcError::Closed);
            }
            if let Some(p) = capture
                .clipboard
                .lock()
                .map_err(|_| LocalIpcError::Protocol)?
                .as_ref()
                .filter(|p| !p.sender.is_closed())
                .cloned()
            {
                return Ok(p);
            }
            changed.await;
        }
    })
    .await
    .map_err(|_| DesktopError::Timeout)?
    .map_err(unavailable)?;
    let (reply, result) = oneshot::channel();
    channel
        .sender
        .send(Request {
            config: config.clone(),
            operation,
            reply,
        })
        .await
        .map_err(unavailable)?;
    tokio::select! {
        _ = revoked(&capture, &config) => Err(DesktopError::Closed),
        result = result => result.map_err(unavailable)?.map_err(unavailable),
    }
}
async fn send_image<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    image: Option<&DesktopClipboardImage>,
) -> Result<(), LocalIpcError> {
    if let Some(image) = image {
        for chunk in image.data.chunks(DESKTOP_CLIPBOARD_CHUNK_BYTES) {
            socket.send(Message::Binary(chunk.to_vec().into())).await?;
        }
    }
    Ok(())
}
async fn read_image<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    image: Option<&mut DesktopClipboardImage>,
) -> Result<(), LocalIpcError> {
    if let Some(image) = image {
        image
            .validate_header()
            .map_err(|_| LocalIpcError::Protocol)?;
        while image.data.len() < image.bytes as usize {
            match receive_frame(socket).await? {
                Message::Binary(bytes)
                    if !bytes.is_empty()
                        && bytes.len()
                            <= DESKTOP_CLIPBOARD_CHUNK_BYTES
                                .min(image.bytes as usize - image.data.len()) =>
                {
                    image.data.extend_from_slice(&bytes)
                }
                _ => return Err(LocalIpcError::Protocol),
            }
        }
        image.validate().map_err(|_| LocalIpcError::Protocol)?;
    }
    Ok(())
}
pub(in crate::local_ipc) async fn serve_registered(
    mut socket: WebSocketStream<TcpStream>,
    frame: Value,
) -> Result<(), LocalIpcError> {
    if frame["version"].as_u64() != Some(DESKTOP_CLIPBOARD_VERSION.into()) {
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
                && p.clipboard_supported
                && !p.retired.load(Ordering::Acquire)
                && p.identity.same_process(&identity)
        })
        .cloned()
        .ok_or(LocalIpcError::Authentication)?;
    let (sender, mut requests) = mpsc::channel::<Request>(1);
    let channel = Arc::new(ClipboardProvider { sender });
    {
        let mut slot = capture
            .clipboard
            .lock()
            .map_err(|_| LocalIpcError::Protocol)?;
        if slot.is_some() {
            return Err(LocalIpcError::Protocol);
        }
        *slot = Some(channel.clone());
    }
    let _registration = Registration(capture.clone(), channel);
    send_json(
        &mut socket,
        &json!({"type":"media_clipboard_registered","version":DESKTOP_CLIPBOARD_VERSION}),
    )
    .await?;
    CHANGED.notify_waiters();
    loop {
        let Request {
            config,
            operation,
            mut reply,
        } = tokio::select! {
            _ = input::capture_ended(&capture) => return Ok(()),
            _ = receive_frame(&mut socket) => return Err(LocalIpcError::Closed),
            r = requests.recv() => r.ok_or(LocalIpcError::Closed)?,
        };
        if reply.is_closed() {
            continue;
        }
        if !authorized(&capture, &config) {
            let _ = reply.send(Err(LocalIpcError::Closed));
            continue;
        }
        let work = async {
            send_json(&mut socket, &json!({"config":config,"request":operation})).await?;
            send_image(&mut socket, operation.image()).await?;
            let response: Result<DesktopClipboardSnapshot, String> =
                read_clipboard_control(&mut socket).await?;
            let mut snapshot = response.map_err(LocalIpcError::Remote)?;
            read_image(&mut socket, snapshot.image.as_mut()).await?;
            snapshot.validate().map_err(|_| LocalIpcError::Protocol)?;
            Ok::<_, LocalIpcError>(snapshot)
        };
        let result = tokio::select! {
            biased;
            _ = revoked(&capture, &config) => return Ok(()),
            _ = reply.closed() => return Ok(()),
            result = tokio::time::timeout(Duration::from_secs(DESKTOP_CLIPBOARD_TIMEOUT_SECS), work) => result.map_err(|_| LocalIpcError::Timeout)?,
        };
        let failed = result
            .as_ref()
            .is_err_and(|e| !matches!(e, LocalIpcError::Remote(_)));
        let _ = reply.send(result);
        if failed {
            return Ok(());
        }
    }
}
pub(super) async fn serve_helper(
    backend: &dyn DesktopBackend,
    provider_id: u64,
) -> Result<(), LocalIpcError> {
    if !backend.supports_clipboard() {
        return std::future::pending().await;
    }
    loop {
        if let Ok(socket) = connect_local().await {
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
    send_json(&mut socket,&json!({"type":"register_media_clipboard_helper","version":DESKTOP_CLIPBOARD_VERSION,"provider_id":provider_id,"process_id":std::process::id()})).await?;
    tokio::time::timeout(AUTH_TIMEOUT, async {
        loop {
            let frame: Value = read_clipboard_control(&mut socket).await?;
            match frame["type"].as_str() {
                Some("verify_helper_identity") => helper_identity::answer(&frame).await?,
                Some("media_clipboard_registered")
                    if frame["version"].as_u64() == Some(DESKTOP_CLIPBOARD_VERSION.into()) =>
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
    #[derive(Deserialize)]
    struct Command {
        config: DesktopVideoConfig,
        request: DesktopClipboardOperation,
    }
    loop {
        let mut command: Command = read_clipboard_control(&mut socket).await?;
        read_image(&mut socket, command.request.image_mut()).await?;
        command
            .request
            .validate()
            .map_err(|_| LocalIpcError::Protocol)?;
        let result = tokio::select! {
            _ = receive_frame(&mut socket) => return Err(LocalIpcError::Closed),
            result = backend.clipboard(command.config, command.request) => result.map_err(|e| e.to_string()),
        };
        send_json(&mut socket, &serde_json::to_value(&result)?).await?;
        if let Ok(snapshot) = &result {
            send_image(&mut socket, snapshot.image.as_ref()).await?;
        }
    }
}

async fn read_clipboard_control<
    S: AsyncRead + AsyncWrite + Unpin,
    T: serde::de::DeserializeOwned,
>(
    socket: &mut WebSocketStream<S>,
) -> Result<T, LocalIpcError> {
    match receive_frame(socket).await? {
        Message::Text(text) if text.len() <= pab_protocol::DESKTOP_CLIPBOARD_JSON_BYTES => {
            Ok(serde_json::from_str(&text)?)
        }
        _ => Err(LocalIpcError::Protocol),
    }
}
