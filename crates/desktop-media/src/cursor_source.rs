//! Cursor-only native worker. No desktop surface crosses this pipe. Cancellation
//! drops/reaps the worker; a retained cursor cannot extend its capture lease.
use super::*;
use std::time::Duration;
use tokio::sync::watch;

pub(super) struct CaptureLease {
    config: DesktopVideoConfig,
    pub(super) ended: watch::Sender<bool>,
    #[cfg(windows)]
    capture_token: String,
    #[cfg(target_os = "linux")]
    pub(super) channel: Option<super::linux_cursor_channel::Channel>,
}
impl CaptureLease {
    pub(super) fn new(
        config: DesktopVideoConfig,
        #[cfg(windows)] capture_token: String,
        #[cfg(target_os = "linux")] channel: Option<super::linux_cursor_channel::Channel>,
    ) -> Self {
        Self {
            config,
            ended: watch::channel(false).0,
            #[cfg(windows)]
            capture_token,
            #[cfg(target_os = "linux")]
            channel,
        }
    }
}
async fn ended(ended: &mut watch::Receiver<bool>) {
    loop {
        if *ended.borrow_and_update() {
            return;
        }
        if ended.changed().await.is_err() {
            return;
        }
    }
}
pub(super) async fn open(
    backend: &NativeBackend,
    config: DesktopVideoConfig,
) -> Result<Box<dyn DesktopCursorSource>> {
    config.validate().map_err(media_error)?;
    let lease = backend
        .capture
        .lock()
        .map_err(|_| media_error("capture state unavailable"))?
        .as_ref()
        .and_then(std::sync::Weak::upgrade)
        .filter(|lease| lease.config == config && !*lease.ended.borrow())
        .ok_or(DesktopError::Closed)?;
    let stopped = lease.ended.subscribe();
    #[cfg(windows)]
    let capture_token = lease.capture_token.clone();
    #[cfg(target_os = "linux")]
    let channel = lease.channel.as_ref().map(|channel| channel.path.clone());
    // Do not hold a strong lease: dropping video must invalidate this cursor,
    // even when its caller retains the source without requesting another update.
    drop(lease);
    #[cfg(target_os = "linux")]
    {
        if let Some(path) = channel {
            return open_unix(backend, config, stopped, path).await;
        }
        if !super::linux_cursor_channel::x11_selected() || !config.screen.id.starts_with("x11:") {
            return Err(media_error(
                "independent cursor unavailable for this capture",
            ));
        }
    }
    {
        let mut stopped = stopped;
        let start = async {
            let permit = backend.cursor_gate.clone().lock_owned().await;
            #[allow(unused_mut)]
            let mut args = vec!["cursor".into(), config.screen.id.clone()];
            #[cfg(windows)]
            args.push(capture_token);
            let mut process =
                MediaChild::spawn(&backend.executable, &backend.prefix, &args, Some(permit))
                    .map_err(media_error)?;
            {
                let mut input = process.input.lock().await;
                input
                    .write_i32_le(config.screen.x)
                    .await
                    .map_err(media_error)?;
                input
                    .write_i32_le(config.screen.y)
                    .await
                    .map_err(media_error)?;
                input
                    .write_u32_le(config.screen.desktop_width)
                    .await
                    .map_err(media_error)?;
                input
                    .write_u32_le(config.screen.desktop_height)
                    .await
                    .map_err(media_error)?;
                input
                    .write_u32_le(match config.screen.rotation {
                        DesktopRotation::Identity => 0,
                        DesktopRotation::Clockwise90 => 1,
                        DesktopRotation::Clockwise180 => 2,
                        DesktopRotation::Clockwise270 => 3,
                    })
                    .await
                    .map_err(media_error)?;
            }
            process.ready().await.map_err(media_error)?;
            if process
                .output
                .as_mut()
                .unwrap()
                .read_u32_le()
                .await
                .map_err(media_error)?
                != 1
            {
                return Err(media_error("native cursor IPC version mismatch"));
            }
            Ok(process)
        };
        let process = tokio::select! {
            biased;
            _ = ended(&mut stopped) => return Err(DesktopError::Closed),
            result = tokio::time::timeout(Duration::from_secs(5), start) => result.map_err(|_| DesktopError::Timeout)??,
        };
        // Own the process in a task which observes capture retirement also while
        // the caller is idle. The one-slot request queue has no position history.
        let (requests, receiver) = tokio::sync::mpsc::channel(1);
        let task = tokio::spawn(run(process, config, stopped, receiver));
        Ok(Box::new(NativeCursor { requests, task }))
    }
}
type Reply = tokio::sync::oneshot::Sender<Result<CursorUpdate>>;
#[cfg(target_os = "linux")]
async fn open_unix(
    backend: &NativeBackend,
    config: DesktopVideoConfig,
    mut stopped: watch::Receiver<bool>,
    path: PathBuf,
) -> Result<Box<dyn DesktopCursorSource>> {
    let start = async {
        let permit = backend.cursor_gate.clone().lock_owned().await;
        let mut stream = tokio::net::UnixStream::connect(path)
            .await
            .map_err(media_error)?;
        stream
            .write_i32_le(config.screen.x)
            .await
            .map_err(media_error)?;
        stream
            .write_i32_le(config.screen.y)
            .await
            .map_err(media_error)?;
        stream
            .write_u32_le(config.screen.desktop_width)
            .await
            .map_err(media_error)?;
        stream
            .write_u32_le(config.screen.desktop_height)
            .await
            .map_err(media_error)?;
        if config.screen.rotation != DesktopRotation::Identity {
            return Err(media_error("Linux capture must be oriented"));
        }
        stream.write_u32_le(0).await.map_err(media_error)?;
        if stream.read_u32_le().await.map_err(media_error)? != MAGIC
            || stream.read_u32_le().await.map_err(media_error)? != 1
        {
            return Err(media_error("native cursor IPC version mismatch"));
        }
        Ok((stream, permit))
    };
    let (mut stream, permit) = tokio::select! {
        biased;
        _=ended(&mut stopped)=>return Err(DesktopError::Closed),
        result=tokio::time::timeout(Duration::from_secs(5),start)=>result.map_err(|_|DesktopError::Timeout)??,
    };
    let (requests, mut receiver) = tokio::sync::mpsc::channel::<Reply>(1);
    let task = tokio::spawn(async move {
        let _permit = permit;
        let mut sequence = 1;
        let mut image_id = 0;
        loop {
            let mut reply = tokio::select! {
                biased;
                _=ended(&mut stopped)=>return,
                request=receiver.recv()=>match request { Some(reply)=>reply,None=>return },
            };
            let operation = async {
                stream.write_u8(1).await.map_err(media_error)?;
                read_packet(
                    &mut stream,
                    &config,
                    sequence,
                    image_id,
                    Duration::from_secs(5),
                )
                .await
            };
            let result = tokio::select! {
                biased;
                _=ended(&mut stopped)=>return,
                _=reply.closed()=>return,
                result=operation=>result,
            };
            let failed = result.is_err();
            if let Ok(update) = &result {
                image_id = update.header.image_id;
                sequence += 1;
            }
            if reply.send(result).is_err() || failed {
                return;
            }
        }
    });
    Ok(Box::new(NativeCursor { requests, task }))
}
struct NativeCursor {
    requests: tokio::sync::mpsc::Sender<Reply>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for NativeCursor {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl DesktopCursorSource for NativeCursor {
    fn next_update(&mut self) -> MediaFuture<'_, CursorUpdate> {
        Box::pin(async {
            let (reply, result) = tokio::sync::oneshot::channel();
            self.requests
                .send(reply)
                .await
                .map_err(|_| DesktopError::Closed)?;
            result.await.map_err(|_| DesktopError::Closed)?
        })
    }
}
async fn run(
    mut process: MediaChild,
    config: DesktopVideoConfig,
    mut stopped: watch::Receiver<bool>,
    mut requests: tokio::sync::mpsc::Receiver<Reply>,
) {
    let mut sequence = 1;
    let mut image_id = 0;
    loop {
        let mut reply = tokio::select! {
            biased;
            _ = ended(&mut stopped) => return,
            request = requests.recv() => match request { Some(reply) => reply, None => return },
        };
        let result = tokio::select! {
            biased;
            _ = ended(&mut stopped) => return,
            _ = reply.closed() => return,
            result = read_update(&mut process, &config, sequence, image_id) => result,
        };
        let failed = result.is_err();
        if let Ok(update) = &result {
            image_id = update.header.image_id;
            sequence += 1;
        }
        if reply.send(result).is_err() || failed {
            return;
        }
    }
}
async fn read_update(
    process: &mut MediaChild,
    config: &DesktopVideoConfig,
    sequence: u64,
    image_id: u64,
) -> Result<CursorUpdate> {
    process
        .input
        .lock()
        .await
        .write_u8(1)
        .await
        .map_err(media_error)?;
    read_packet(
        process.output.as_mut().unwrap(),
        config,
        sequence,
        image_id,
        Duration::from_secs(5),
    )
    .await
}
async fn read_packet(
    output: &mut (impl tokio::io::AsyncRead + Unpin),
    config: &DesktopVideoConfig,
    sequence: u64,
    image_id: u64,
    timeout: Duration,
) -> Result<CursorUpdate> {
    sequence
        .checked_add(1)
        .ok_or(DesktopError::Protocol("cursor sequence exhausted"))?;
    // The first byte may wait for a real cursor change indefinitely. Once it
    // arrives, finish the bounded packet within a finite deadline.
    if output.read_u8().await.map_err(media_error)? != 1 {
        return Err(media_error("invalid native cursor update"));
    }
    tokio::time::timeout(timeout, async {
        let version = output.read_u16_le().await.map_err(media_error)?;
        let native_sequence = output.read_u64_le().await.map_err(media_error)?;
        let visible = output.read_u8().await.map_err(media_error)?;
        if visible > 1 {
            return Err(media_error("invalid native cursor visibility"));
        }
        let x = output.read_i32_le().await.map_err(media_error)?;
        let y = output.read_i32_le().await.map_err(media_error)?;
        let next_image_id = output.read_u64_le().await.map_err(media_error)?;
        let format = output.read_u8().await.map_err(media_error)?;
        let image = match format {
            0 => None,
            1 | 2 => Some(DesktopCursorImage {
                format: if format == 1 {
                    DesktopCursorFormat::PremultipliedBgra
                } else {
                    DesktopCursorFormat::AndXorBgra
                },
                width: output.read_u32_le().await.map_err(media_error)?,
                height: output.read_u32_le().await.map_err(media_error)?,
                hotspot_x: output.read_u32_le().await.map_err(media_error)?,
                hotspot_y: output.read_u32_le().await.map_err(media_error)?,
            }),
            _ => return Err(media_error("invalid native cursor format")),
        };
        let header = DesktopCursorHeader {
            version,
            session_id: config.session_id,
            generation: config.generation,
            sequence: native_sequence,
            position: (visible == 1).then_some(DesktopCursorPosition { x, y }),
            image_id: next_image_id,
            image,
        };
        let size = header
            .validate(config, sequence, image_id)
            .map_err(media_error)?;
        let mut bytes = vec![0; size];
        output.read_exact(&mut bytes).await.map_err(media_error)?;
        Ok(CursorUpdate { header, bytes })
    })
    .await
    .map_err(|_| DesktopError::Timeout)?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> DesktopVideoConfig {
        DesktopVideoConfig {
            session_id: RequestId::new(),
            generation: 7,
            fps: 15,
            codec: DesktopCodec::H264AnnexB,
            color: DesktopColor::Bt709Limited,
            screen: DesktopScreen {
                id: "generated".into(),
                name: "Generated".into(),
                x: -1920,
                y: -100,
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
    fn packet(sequence: u64, visible: u8, image_id: u64, format: u8, width: u32) -> Vec<u8> {
        let mut bytes = vec![1];
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&sequence.to_le_bytes());
        bytes.push(visible);
        bytes.extend_from_slice(&(-1900i32).to_le_bytes());
        bytes.extend_from_slice(&(-50i32).to_le_bytes());
        bytes.extend_from_slice(&image_id.to_le_bytes());
        bytes.push(format);
        if format != 0 {
            for value in [width, 1, 0, 0] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes
    }
    #[tokio::test]
    async fn native_cursor_packet_image_hide_move_keeps_exact_framing() {
        let config = config();
        let (mut input, mut output) = tokio::io::duplex(1024);
        let mut bytes = packet(1, 1, 1, 1, 1);
        bytes.extend_from_slice(&[1, 2, 3, 255]);
        bytes.extend(packet(2, 0, 1, 0, 0));
        bytes.extend(packet(3, 1, 1, 0, 0));
        input.write_all(&bytes).await.unwrap();
        let first = read_packet(&mut output, &config, 1, 0, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(first.header.session_id, config.session_id);
        assert_eq!(first.header.generation, 7);
        assert_eq!(
            first.header.position,
            Some(DesktopCursorPosition { x: -1900, y: -50 })
        );
        assert_eq!(first.bytes, [1, 2, 3, 255]);
        let hidden = read_packet(&mut output, &config, 2, 1, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(hidden.header.position.is_none() && hidden.bytes.is_empty());
        let moved = read_packet(&mut output, &config, 3, 1, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(
            moved.header.image.is_none()
                && moved.header.position.is_some()
                && moved.bytes.is_empty()
        );
    }
    #[tokio::test]
    async fn native_cursor_rejects_header_before_waiting_or_allocating_payload() {
        for bytes in [
            packet(2, 1, 1, 1, 1),
            packet(1, 2, 1, 1, 1),
            packet(1, 1, 1, 9, 1),
            packet(1, 1, 1, 1, 513),
            packet(1, 1, 99, 0, 0),
            packet(1, 1, 1, 1, u32::MAX),
        ] {
            let (mut input, mut output) = tokio::io::duplex(1024);
            input.write_all(&bytes).await.unwrap();
            assert!(tokio::time::timeout(
                Duration::from_millis(200),
                read_packet(&mut output, &config(), 1, 0, Duration::from_secs(5))
            )
            .await
            .unwrap()
            .is_err());
        }
    }
    #[tokio::test]
    async fn native_cursor_partial_payload_has_deadline_but_idle_wait_can_cancel() {
        let (input, mut output) = tokio::io::duplex(1024);
        let config = config();
        // No artificial update is generated on a stationary desktop.
        assert!(tokio::time::timeout(
            Duration::from_millis(50),
            read_packet(&mut output, &config, 1, 0, Duration::from_millis(20))
        )
        .await
        .is_err());
        // The owning task retires the process after cancelled IO. Use a fresh
        // pipe for partial-payload tests; never reuse an ambiguous live worker.
        drop(input);
        drop(output);
        let (mut input, mut output) = tokio::io::duplex(1024);
        let mut bytes = packet(1, 1, 1, 1, 1);
        bytes.extend_from_slice(&[1, 2]);
        input.write_all(&bytes).await.unwrap();
        assert!(matches!(
            read_packet(&mut output, &config, 1, 0, Duration::from_millis(20)).await,
            Err(DesktopError::Timeout)
        ));
    }
}
