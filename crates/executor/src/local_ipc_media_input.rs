//! Input uses another instance of the existing authenticated helper transport.
//! Pairing requires the capture registration's kernel-verified process identity;
//! an advertised provider ID alone never grants access. Static capture cannot
//! block input. No input is routed through the legacy window request queue.
use super::*;
use pab_protocol::RemoteDesktopInput;
use pab_remote_desktop::DesktopInputSink;

const INPUT_VERSION: u16 = 1;
#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum InputOperation {
    Start { config: DesktopVideoConfig },
    Event { event: RemoteDesktopInput },
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
enum InputReply {
    Applied,
    Failed,
}
struct InputRequest {
    operation: InputOperation,
    stop: Option<oneshot::Receiver<()>>,
    reply: oneshot::Sender<Result<(), LocalIpcError>>,
}
pub(super) struct InputProvider {
    sender: mpsc::Sender<InputRequest>,
    claimed: AtomicBool,
}
struct Registration {
    capture: Arc<Provider>,
    input: Arc<InputProvider>,
}
impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut current) = self.capture.input.lock() {
            if current
                .as_ref()
                .is_some_and(|p| Arc::ptr_eq(p, &self.input))
            {
                *current = None;
            }
        }
        CHANGED.notify_waiters();
    }
}
struct InputSink {
    capture: Arc<Provider>,
    input: Arc<InputProvider>,
    _stop: oneshot::Sender<()>,
}
impl DesktopInputSink for InputSink {
    fn apply(&mut self, event: RemoteDesktopInput) -> MediaFuture<'_, ()> {
        Box::pin(async move {
            event.validate().map_err(unavailable)?;
            if self.capture.retired.load(Ordering::Acquire) {
                return Err(DesktopError::Closed);
            }
            request(&self.input, InputOperation::Event { event }, None)
                .await
                .map_err(unavailable)
        })
    }
}
async fn request(
    input: &InputProvider,
    operation: InputOperation,
    stop: Option<oneshot::Receiver<()>>,
) -> Result<(), LocalIpcError> {
    let (reply, result) = oneshot::channel();
    input
        .sender
        .send(InputRequest {
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
) -> Result<Box<dyn DesktopInputSink>, DesktopError> {
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
    if !capture.input_supported {
        return Err(unavailable("capture helper does not support input"));
    }
    let input = tokio::time::timeout(AUTH_TIMEOUT, async {
        loop {
            let changed = CHANGED.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if capture.retired.load(Ordering::Acquire) {
                return Err(LocalIpcError::Closed);
            }
            if let Some(input) = capture
                .input
                .lock()
                .map_err(|_| LocalIpcError::Protocol)?
                .as_ref()
                .filter(|input| !input.sender.is_closed() && !input.claimed.load(Ordering::Acquire))
                .cloned()
            {
                if !input.claimed.swap(true, Ordering::AcqRel) {
                    return Ok(input);
                }
            }
            changed.await;
        }
    })
    .await
    .map_err(|_| DesktopError::Timeout)?
    .map_err(unavailable)?;
    let (stop, stopped) = oneshot::channel();
    // If startup is cancelled, the stop sender is dropped and retires the socket.
    let sink = InputSink {
        capture,
        input,
        _stop: stop,
    };
    request(&sink.input, InputOperation::Start { config }, Some(stopped))
        .await
        .map_err(unavailable)?;
    Ok(Box::new(sink))
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
    if frame["version"].as_u64() != Some(INPUT_VERSION.into()) {
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
                && p.input_supported
                && !p.retired.load(Ordering::Acquire)
                && p.identity.same_process(&identity)
        })
        .cloned()
        .ok_or(LocalIpcError::Authentication)?;
    let (sender, mut requests) = mpsc::channel(1);
    let input = Arc::new(InputProvider {
        sender,
        claimed: AtomicBool::new(false),
    });
    {
        let mut current = capture.input.lock().map_err(|_| LocalIpcError::Protocol)?;
        if current.is_some() {
            return Err(LocalIpcError::Protocol);
        }
        *current = Some(input.clone());
    }
    let _registration = Registration {
        capture: capture.clone(),
        input,
    };
    send_json(
        &mut socket,
        &json!({"type":"media_input_registered","version":INPUT_VERSION}),
    )
    .await?;
    CHANGED.notify_waiters();
    let mut stop = None;
    loop {
        let request = tokio::select! {
            biased;
            _ = capture_ended(&capture) => return Ok(()),
            _ = stopped(&mut stop) => return Ok(()),
            _ = receive_frame(&mut socket) => return Err(LocalIpcError::Closed),
            request = requests.recv() => request.ok_or(LocalIpcError::Closed)?,
        };
        let InputRequest {
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
        if let InputOperation::Start { config } = &operation {
            if stop.is_some()
                || capture
                    .active_config
                    .lock()
                    .map_err(|_| LocalIpcError::Protocol)?
                    .as_ref()
                    != Some(config)
            {
                return Err(LocalIpcError::Protocol);
            }
            stop = new_stop;
        } else if stop.is_none() {
            return Err(LocalIpcError::Protocol);
        }
        let exchange = async {
            send_json(&mut socket, &serde_json::to_value(operation)?).await?;
            match read_control::<_, InputReply>(&mut socket).await? {
                InputReply::Applied => Ok(()),
                InputReply::Failed => {
                    Err(LocalIpcError::Remote("remote desktop input failed".into()))
                }
            }
        };
        let result = tokio::select! {
            biased;
            _ = capture_ended(&capture) => return Ok(()),
            _ = stopped(&mut stop) => return Ok(()),
            _ = reply.closed() => return Ok(()),
            result = exchange => result,
        };
        let failed = result.is_err();
        if reply.send(result).is_err() || failed {
            return Ok(());
        }
    }
}
pub(super) async fn serve_helper(
    backend: &dyn DesktopBackend,
    provider_id: u64,
) -> Result<(), LocalIpcError> {
    if !backend.supports_input() {
        return std::future::pending().await;
    }
    loop {
        if let Ok(socket) = connect_local().await {
            // No input data or errors containing events are logged.
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
    send_json(&mut socket, &json!({"type":"register_media_input_helper","version":INPUT_VERSION,"provider_id":provider_id,"process_id":std::process::id()})).await?;
    tokio::time::timeout(AUTH_TIMEOUT, async {
        loop {
            let frame: Value = read_control(&mut socket).await?;
            match frame["type"].as_str() {
                Some("verify_helper_identity") => helper_identity::answer(&frame).await?,
                Some("media_input_registered")
                    if frame["version"].as_u64() == Some(INPUT_VERSION.into()) =>
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
    let mut sink: Option<Box<dyn DesktopInputSink>> = None;
    loop {
        let operation: InputOperation = read_control(&mut socket).await?;
        let work = async {
            match operation {
                InputOperation::Start { config } if sink.is_none() => {
                    config.validate().map_err(unavailable)?;
                    sink = Some(backend.input(config).await?);
                    Ok(())
                }
                InputOperation::Event { event } if sink.is_some() => {
                    event.validate().map_err(unavailable)?;
                    sink.as_mut().unwrap().apply(event).await
                }
                _ => Err(DesktopError::Closed),
            }
        };
        let result = tokio::select! {
            result = work => result,
            _ = receive_frame(&mut socket) => return Err(LocalIpcError::Closed),
        };
        send_json(
            &mut socket,
            &serde_json::to_value(if result.is_ok() {
                InputReply::Applied
            } else {
                InputReply::Failed
            })?,
        )
        .await?;
        if result.is_err() {
            return Err(LocalIpcError::Closed);
        }
    }
}
