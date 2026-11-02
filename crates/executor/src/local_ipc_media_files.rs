//! Independent authenticated file-clipboard socket. Its helper must be the
//! kernel-verified active capture process, with a live input lease.
use super::*;
use pab_protocol::{
    FILE_CLIPBOARD_CHUNK, FILE_CLIPBOARD_HEADER, FILE_CLIPBOARD_VERSION,
    FileClipboardOperation as Op, FileClipboardSnapshot as Snapshot,
};
struct Request {
    config: DesktopVideoConfig,
    operation: Op,
    reply: oneshot::Sender<Result<Snapshot, LocalIpcError>>,
}
pub(super) struct FileProvider {
    sender: mpsc::Sender<Request>,
}
struct Registration(Arc<Provider>, Arc<FileProvider>);
impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.0.files.lock() {
            if slot.as_ref().is_some_and(|p| Arc::ptr_eq(p, &self.1)) {
                *slot = None;
            }
        }
        CHANGED.notify_waiters();
    }
}
pub(super) async fn exchange(
    config: DesktopVideoConfig,
    operation: Op,
) -> Result<Snapshot, DesktopError> {
    operation.validate().map_err(unavailable)?;
    let capture = providers()
        .lock()
        .map_err(|_| unavailable("file_clipboard_unavailable"))?
        .iter()
        .find(|p| p.file_clipboard_supported && clipboard::authorized(p, &config))
        .cloned()
        .ok_or(DesktopError::Closed)?;
    let channel = tokio::time::timeout(AUTH_TIMEOUT, async {
        loop {
            let changed = CHANGED.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if !clipboard::authorized(&capture, &config) {
                return Err(LocalIpcError::Closed);
            }
            if let Some(channel) = capture
                .files
                .lock()
                .map_err(|_| LocalIpcError::Protocol)?
                .as_ref()
                .filter(|c| !c.sender.is_closed())
                .cloned()
            {
                return Ok(channel);
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
    tokio::select! {_=clipboard::revoked(&capture,&config)=>Err(DesktopError::Closed),value=result=>value.map_err(unavailable)?.map_err(unavailable)}
}
async fn control<S: AsyncRead + AsyncWrite + Unpin, T: serde::de::DeserializeOwned>(
    socket: &mut WebSocketStream<S>,
) -> Result<T, LocalIpcError> {
    match receive_frame(socket).await? {
        Message::Text(text) if text.len() <= FILE_CLIPBOARD_HEADER => {
            Ok(serde_json::from_str(&text)?)
        }
        _ => Err(LocalIpcError::Protocol),
    }
}
async fn read_payload<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    length: u32,
) -> Result<Vec<u8>, LocalIpcError> {
    if length as usize > FILE_CLIPBOARD_CHUNK {
        return Err(LocalIpcError::Protocol);
    }
    let mut data = Vec::with_capacity(length as usize);
    while data.len() < length as usize {
        match receive_frame(socket).await? {
            Message::Binary(bytes)
                if !bytes.is_empty() && bytes.len() <= length as usize - data.len() =>
            {
                data.extend_from_slice(&bytes)
            }
            _ => return Err(LocalIpcError::Protocol),
        }
    }
    Ok(data)
}
async fn send_payload<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    data: &[u8],
) -> Result<(), LocalIpcError> {
    if data.len() > FILE_CLIPBOARD_CHUNK {
        return Err(LocalIpcError::Protocol);
    }
    if !data.is_empty() {
        socket.send(Message::Binary(data.to_vec().into())).await?;
    }
    Ok(())
}
fn verify(snapshot: &Snapshot) -> Result<(), LocalIpcError> {
    use sha2::{Digest, Sha256};
    snapshot.validate().map_err(|_| LocalIpcError::Protocol)?;
    if snapshot.data.len() != snapshot.bytes as usize
        || snapshot.bytes > 0
            && snapshot.sha256.as_deref()
                != Some(format!("{:x}", Sha256::digest(&snapshot.data)).as_str())
    {
        return Err(LocalIpcError::Protocol);
    }
    Ok(())
}
pub(in crate::local_ipc) async fn serve_registered(
    mut socket: WebSocketStream<TcpStream>,
    frame: Value,
) -> Result<(), LocalIpcError> {
    socket.get_ref().set_nodelay(true)?;
    if frame["version"].as_u64() != Some(FILE_CLIPBOARD_VERSION.into()) {
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
                && p.file_clipboard_supported
                && !p.retired.load(Ordering::Acquire)
                && p.identity.same_process(&identity)
        })
        .cloned()
        .ok_or(LocalIpcError::Authentication)?;
    let (sender, mut requests) = mpsc::channel::<Request>(1);
    let channel = Arc::new(FileProvider { sender });
    {
        let mut slot = capture.files.lock().map_err(|_| LocalIpcError::Protocol)?;
        if slot.is_some() {
            return Err(LocalIpcError::Protocol);
        }
        *slot = Some(channel.clone());
    }
    let _registration = Registration(capture.clone(), channel);
    send_json(
        &mut socket,
        &json!({"type":"media_file_clipboard_registered","version":FILE_CLIPBOARD_VERSION}),
    )
    .await?;
    CHANGED.notify_waiters();
    loop {
        let Request {
            config,
            operation,
            mut reply,
        } = tokio::select! {_=input::capture_ended(&capture)=>return Ok(()),_=receive_frame(&mut socket)=>return Err(LocalIpcError::Closed),r=requests.recv()=>r.ok_or(LocalIpcError::Closed)?};
        if reply.is_closed() {
            continue;
        }
        if !clipboard::authorized(&capture, &config) {
            let _ = reply.send(Err(LocalIpcError::Closed));
            continue;
        }
        let work = async {
            send_json(&mut socket, &json!({"config":config,"request":operation})).await?;
            send_payload(&mut socket, operation.payload()).await?;
            let response: Result<Snapshot, String> = control(&mut socket).await?;
            let mut snapshot = response.map_err(LocalIpcError::Remote)?;
            snapshot.validate().map_err(|_| LocalIpcError::Protocol)?;
            snapshot.data = read_payload(&mut socket, snapshot.bytes).await?;
            verify(&snapshot)?;
            Ok::<_, LocalIpcError>(snapshot)
        };
        let result = tokio::select! {biased;_=clipboard::revoked(&capture,&config)=>return Ok(()),_=reply.closed()=>return Ok(()),value=tokio::time::timeout(Duration::from_secs(12),work)=>value.map_err(|_|LocalIpcError::Timeout)?};
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
    if !backend.supports_file_clipboard() {
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
    if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
        stream.set_nodelay(true)?;
    }
    send_json(&mut socket,&json!({"type":"register_media_file_clipboard_helper","version":FILE_CLIPBOARD_VERSION,"provider_id":provider_id,"process_id":std::process::id()})).await?;
    tokio::time::timeout(AUTH_TIMEOUT, async {
        loop {
            let frame: Value = control(&mut socket).await?;
            match frame["type"].as_str() {
                Some("verify_helper_identity") => helper_identity::answer(&frame).await?,
                Some("media_file_clipboard_registered")
                    if frame["version"].as_u64() == Some(FILE_CLIPBOARD_VERSION.into()) =>
                {
                    return Ok::<_, LocalIpcError>(());
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
        request: Op,
    }
    loop {
        let mut command: Command = control(&mut socket).await?;
        command
            .request
            .validate()
            .map_err(|_| LocalIpcError::Protocol)?;
        if let Op::Deliver { bytes, data, .. } = &mut command.request {
            *data = read_payload(&mut socket, *bytes).await?;
        }
        let result = tokio::select! {_=receive_frame(&mut socket)=>return Err(LocalIpcError::Closed),value=backend.file_clipboard(command.config,command.request)=>value.map_err(|e|e.to_string())};
        if let Ok(snapshot) = &result {
            verify(snapshot)?;
        }
        send_json(&mut socket, &serde_json::to_value(&result)?).await?;
        if let Ok(snapshot) = &result {
            send_payload(&mut socket, &snapshot.data).await?;
        }
    }
}
