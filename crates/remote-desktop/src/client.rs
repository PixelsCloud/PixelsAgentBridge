use crate::{DesktopError, DesktopVideoReader, Result, VideoFrame};
use pab_protocol::*;
use pab_transport::{PabBiStream, PabConnection, PabFrameWriter};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};

struct Command {
    command: DesktopControlCommand,
    result: oneshot::Sender<Result<DesktopControlReply>>,
}
/// One viewer bound to one already-authenticated connection. Dropping it ends
/// its streams only. No reconnect, account changes or credential storage.
pub struct RemoteDesktopSession {
    connection: PabConnection,
    opened: DesktopOpened,
    timeout: Duration,
    commands: mpsc::Sender<Command>,
    worker: tokio::task::JoinHandle<()>,
    video: Option<DesktopVideoReader>,
    video_keepalive: Option<PabFrameWriter>,
    presentations: watch::Sender<Option<(u64, u64)>>,
    last_received: Arc<AtomicU64>,
    presentation_valid: Arc<Mutex<bool>>,
    closed: bool,
    close_confirmed: bool,
    cursor_version: Option<u16>,
    generation_ended: watch::Sender<bool>,
}
/// A cursor reader never owns the desktop session. Selecting a different
/// generation, closing or dropping its session invalidates this handle even
/// when the caller keeps it alive. Drop closes only its cursor stream.
pub struct RemoteDesktopCursor {
    reader: Option<crate::DesktopCursorReader>,
    keepalive: Option<PabFrameWriter>,
    ended: watch::Receiver<bool>,
    timeout: Duration,
}
impl RemoteDesktopCursor {
    pub fn is_current(&self) -> bool {
        self.reader.is_some()
            && self.keepalive.is_some()
            && !*self.ended.borrow()
            && self.ended.has_changed().is_ok()
    }
    pub async fn next_update(&mut self) -> Result<crate::CursorUpdate> {
        let result = if !self.is_current() {
            Err(DesktopError::Closed)
        } else {
            let reader = self.reader.as_mut().ok_or(DesktopError::Closed)?;
            let keepalive = &mut self.keepalive;
            let timeout = self.timeout;
            tokio::select! {
                biased;
                _ = self.ended.changed() => Err(DesktopError::Closed),
                result = async {
                    let update = reader.receive_after_idle(timeout).await?;
                    // Consumption ACK bounds cursor history to one outstanding
                    // update. Taking the writer also retires a cancelled partial
                    // ACK; a later call cannot interpret/replay it as a new one.
                    let mut writer = keepalive.take().ok_or(DesktopError::Closed)?;
                    tokio::time::timeout(timeout, writer.write(&update.header.sequence.to_le_bytes(), 8))
                        .await.map_err(|_| DesktopError::Timeout)??;
                    *keepalive = Some(writer);
                    Ok(update)
                } => result,
            }
        };
        // Invalidation can race an otherwise complete read. Never hand that
        // old image to the renderer after the session has changed generation.
        let result = if *self.ended.borrow() {
            Err(DesktopError::Closed)
        } else {
            result
        };
        if result.is_err() {
            self.reader = None;
            self.keepalive = None;
        }
        result
    }
}
/// Acknowledges only frames received in this generation. Closing or selecting
/// another screen invalidates existing handles before accepting new output.
#[derive(Clone)]
pub struct RemoteDesktopPresentationControl {
    session: RequestId,
    generation: u64,
    received: Arc<AtomicU64>,
    valid: Arc<Mutex<bool>>,
    presentations: watch::Sender<Option<(u64, u64)>>,
}
impl RemoteDesktopPresentationControl {
    pub fn report_presented(&self, header: &DesktopFrameHeader) -> Result<()> {
        let valid = self.valid.lock().unwrap();
        if !*valid || self.presentations.is_closed() {
            return Err(DesktopError::Closed);
        }
        if header.session_id != self.session
            || header.generation != self.generation
            || header.frame_id == 0
            || header.frame_id > self.received.load(Ordering::Acquire)
        {
            return Err(DesktopError::Protocol(
                "cannot acknowledge an obsolete or unreceived desktop frame",
            ));
        }
        let mut backwards = false;
        self.presentations.send_if_modified(|latest| {
            if let Some((generation, frame_id)) = *latest {
                if generation == header.generation && frame_id >= header.frame_id {
                    backwards = frame_id > header.frame_id;
                    return false;
                }
            }
            *latest = Some((header.generation, header.frame_id));
            true
        });
        if backwards {
            return Err(DesktopError::Protocol(
                "desktop consumption cannot move backwards",
            ));
        }
        Ok(())
    }
}
async fn stream(
    connection: &PabConnection,
    request: RemoteDesktopRequest,
    timeout: Duration,
) -> Result<PabBiStream> {
    let mut stream = connection.open_bi(timeout).await?;
    stream
        .send_frame_json(
            &DeviceTaskRequest::RemoteDesktop {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request,
            },
            timeout,
        )
        .await?;
    Ok(stream)
}
async fn response(stream: &mut PabBiStream, timeout: Duration) -> Result<RemoteDesktopReply> {
    match stream.receive_json(timeout).await? {
        DeviceTaskResponse::RemoteDesktop {
            reply: RemoteDesktopReply::Error { code, message },
        } => Err(DesktopError::reject(code, message)),
        DeviceTaskResponse::RemoteDesktop { reply } => Ok(reply),
        _ => Err(DesktopError::Protocol("unexpected desktop response")),
    }
}
async fn close_id(connection: &PabConnection, id: RequestId, timeout: Duration) -> Result<()> {
    let mut stream = stream(
        connection,
        RemoteDesktopRequest::Close { session_id: id },
        timeout,
    )
    .await?;
    match response(&mut stream, timeout).await? {
        RemoteDesktopReply::Closed => Ok(()),
        _ => Err(DesktopError::Protocol("unexpected desktop close reply")),
    }
}
struct OpenCleanup {
    connection: PabConnection,
    id: RequestId,
    timeout: Duration,
    armed: bool,
}
/// Generation-bound control handle, separate from the video reader. Input must
/// never cancel a partially received video frame, including on a static screen.
/// Keeping this handle does not keep the session alive after its owner closes.
#[derive(Clone)]
pub struct RemoteDesktopInputControl {
    generation: u64,
    supported: bool,
    commands: mpsc::Sender<Command>,
}
impl RemoteDesktopInputControl {
    async fn command(&self, command: DesktopControlCommand) -> Result<()> {
        let (result, reply) = oneshot::channel();
        self.commands
            .send(Command { command, result })
            .await
            .map_err(|_| DesktopError::Closed)?;
        match reply.await.map_err(|_| DesktopError::Closed)?? {
            DesktopControlReply::Applied { .. } => Ok(()),
            _ => Err(DesktopError::Protocol("unexpected input control reply")),
        }
    }
    pub async fn set_enabled(&self, enabled: bool) -> Result<()> {
        if enabled && !self.supported {
            return Err(DesktopError::reject(
                DesktopErrorCode::Unsupported,
                "remote desktop input is unavailable",
            ));
        }
        self.command(DesktopControlCommand::SetInput {
            generation: self.generation,
            enabled,
        })
        .await
    }
    pub async fn send(&self, event: RemoteDesktopInput) -> Result<()> {
        event.validate().map_err(DesktopError::Protocol)?;
        self.command(DesktopControlCommand::Input {
            generation: self.generation,
            event,
        })
        .await
    }
}
impl Drop for OpenCleanup {
    fn drop(&mut self) {
        if self.armed {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let (connection, id, timeout) = (self.connection.clone(), self.id, self.timeout);
                runtime.spawn(async move {
                    let _ = close_id(&connection, id, timeout).await;
                });
            }
        }
    }
}
impl RemoteDesktopSession {
    /// Caller must perform capability negotiation and reuse an authenticated
    /// connection. Server validates every stream against that connection's owner.
    pub async fn open(
        connection: PabConnection,
        screen_id: Option<String>,
        fps: u16,
        timeout: Duration,
    ) -> Result<Self> {
        if timeout.is_zero() || !(1..=60).contains(&fps) {
            return Err(DesktopError::Protocol("invalid desktop timeout or FPS"));
        }
        if let Some(id) = &screen_id {
            validate_desktop_screen_id(id).map_err(DesktopError::Protocol)?;
        }
        let id = RequestId::new();
        let mut cleanup = OpenCleanup {
            connection: connection.clone(),
            id,
            timeout,
            armed: true,
        };
        let mut opening = stream(
            &connection,
            RemoteDesktopRequest::Open {
                version: REMOTE_DESKTOP_SCHEMA_VERSION,
                session_id: id,
                screen_id: screen_id.clone(),
                fps,
            },
            timeout,
        )
        .await?;
        let opened = match response(&mut opening, timeout).await? {
            RemoteDesktopReply::Opened { opened } => opened,
            _ => return Err(DesktopError::Protocol("unexpected desktop open reply")),
        };
        opened.validate().map_err(DesktopError::Protocol)?;
        if opened.config.session_id != id
            || opened.config.generation != 1
            || opened.config.fps != fps
            || screen_id
                .as_ref()
                .is_some_and(|id| id != &opened.config.screen.id)
        {
            return Err(DesktopError::Protocol(
                "desktop open identity/configuration mismatch",
            ));
        }
        let mut control = stream(
            &connection,
            RemoteDesktopRequest::Control { session_id: id },
            timeout,
        )
        .await?;
        if response(&mut control, timeout).await? != RemoteDesktopReply::Ready {
            return Err(DesktopError::Protocol("desktop control rejected"));
        }
        let (commands, requests) = mpsc::channel(8);
        let heartbeat = Duration::from_millis(opened.heartbeat_ms.into());
        let (presentations, presentation_updates) = watch::channel(None);
        let worker = tokio::spawn(control_worker(
            control,
            requests,
            presentation_updates,
            heartbeat,
            timeout,
        ));
        // From this point Self's Drop aborts the worker if video attach fails or
        // this future is cancelled, so heartbeat cannot keep an orphan alive.
        let mut session = Self {
            connection,
            opened,
            timeout,
            commands,
            worker,
            video: None,
            video_keepalive: None,
            presentations,
            last_received: Arc::new(AtomicU64::new(0)),
            presentation_valid: Arc::new(Mutex::new(true)),
            closed: false,
            close_confirmed: false,
            cursor_version: None,
            generation_ended: watch::channel(false).0,
        };
        session.attach_video().await?;
        cleanup.armed = false;
        Ok(session)
    }
    pub fn opened(&self) -> &DesktopOpened {
        &self.opened
    }
    /// The authenticated environment reply supplies this additive capability.
    /// Unknown/absent versions retain existing v2 video and input behavior.
    pub fn with_cursor_support(mut self, version: Option<u16>) -> Self {
        self.cursor_version = version;
        self
    }
    pub fn supports_cursor(&self) -> bool {
        self.cursor_version == Some(DESKTOP_CURSOR_SCHEMA_VERSION)
    }
    /// One stream per generation. Obtain again after selecting/restarting a
    /// screen. It may be read independently while video waits for changes.
    pub async fn open_cursor(&self) -> Result<RemoteDesktopCursor> {
        if self.closed
            || self.worker.is_finished()
            || self.video.is_none()
            || *self.generation_ended.borrow()
        {
            return Err(DesktopError::Closed);
        }
        if !self.supports_cursor() {
            return Err(DesktopError::reject(
                DesktopErrorCode::Unsupported,
                "device has no negotiated independent cursor capability",
            ));
        }
        let config = self.opened.config.clone();
        let mut cursor = stream(
            &self.connection,
            RemoteDesktopRequest::Cursor {
                version: DESKTOP_CURSOR_SCHEMA_VERSION,
                session_id: config.session_id,
                generation: config.generation,
            },
            self.timeout,
        )
        .await?;
        match response(&mut cursor, self.timeout).await? {
            RemoteDesktopReply::CursorReady {
                version: DESKTOP_CURSOR_SCHEMA_VERSION,
            } => {}
            _ => return Err(DesktopError::Protocol("unexpected cursor stream reply")),
        }
        let (keepalive, reader) = cursor.into_framed();
        Ok(RemoteDesktopCursor {
            reader: Some(crate::DesktopCursorReader::new(reader, config)?),
            keepalive: Some(keepalive),
            ended: self.generation_ended.subscribe(),
            timeout: self.timeout,
        })
    }
    pub fn connection_path(&self) -> Option<pab_transport::ConnectionPath> {
        self.connection.selected_path()
    }
    /// Acquire again after screen selection/restart. Old handles cannot send
    /// input to a newer generation or accidentally disable its input lease.
    pub fn input_control(&self) -> RemoteDesktopInputControl {
        RemoteDesktopInputControl {
            generation: self.opened.config.generation,
            supported: self.opened.input_supported,
            commands: self.commands.clone(),
        }
    }
    /// Separate generation-bound handle for asynchronous decoder output. It
    /// can acknowledge rendered frames while another future reads video.
    pub fn presentation_control(&self) -> RemoteDesktopPresentationControl {
        RemoteDesktopPresentationControl {
            session: self.opened.config.session_id,
            generation: self.opened.config.generation,
            received: self.last_received.clone(),
            valid: self.presentation_valid.clone(),
            presentations: self.presentations.clone(),
        }
    }
    /// Confirms enabling/retiring this generation's input route. It does not
    /// confirm that the OS processed releases or an application acted on input.
    pub async fn set_input_enabled(&self, enabled: bool) -> Result<()> {
        self.input_control().set_enabled(enabled).await
    }
    pub async fn send_input(&self, event: RemoteDesktopInput) -> Result<()> {
        self.input_control().send(event).await
    }
    async fn attach_video(&mut self) -> Result<()> {
        let config = self.opened.config.clone();
        let mut video = stream(
            &self.connection,
            RemoteDesktopRequest::Video {
                session_id: config.session_id,
                generation: config.generation,
            },
            self.timeout,
        )
        .await?;
        if response(&mut video, self.timeout).await? != RemoteDesktopReply::Ready {
            return Err(DesktopError::Protocol("desktop video rejected"));
        }
        let (keepalive, reader) = video.into_framed();
        self.video = Some(DesktopVideoReader::new(reader, config)?);
        self.video_keepalive = Some(keepalive);
        Ok(())
    }
    async fn command(&self, command: DesktopControlCommand) -> Result<DesktopControlReply> {
        if self.closed || self.worker.is_finished() {
            return Err(DesktopError::Closed);
        }
        let (result, reply) = oneshot::channel();
        self.commands
            .send(Command { command, result })
            .await
            .map_err(|_| DesktopError::Closed)?;
        reply.await.map_err(|_| DesktopError::Closed)?
    }
    pub async fn next_frame(&mut self, timeout: Duration) -> Result<VideoFrame> {
        if self.closed || self.worker.is_finished() {
            return Err(DesktopError::Closed);
        }
        let frame = self
            .video
            .as_mut()
            .ok_or(DesktopError::Closed)?
            .receive(timeout)
            .await?;
        self.last_received
            .store(frame.header.frame_id, Ordering::Release);
        Ok(frame)
    }
    /// Product viewers use this for static desktops. Control heartbeat failure
    /// still terminates the wait; lack of a changed image does not.
    pub async fn next_frame_when_changed(&mut self) -> Result<VideoFrame> {
        if self.closed || self.worker.is_finished() {
            return Err(DesktopError::Closed);
        }
        let reader = self.video.as_mut().ok_or(DesktopError::Closed)?;
        let frame = tokio::select! {
            frame = reader.receive_after_idle(self.timeout) => frame?,
            _ = &mut self.worker => return Err(DesktopError::Closed),
        };
        self.last_received
            .store(frame.header.frame_id, Ordering::Release);
        Ok(frame)
    }
    /// Queue cumulative decode/display consumption without blocking the render
    /// loop on an RTT for each frame. Updates coalesce while control is in flight.
    /// This confirms local queuing, not remote delivery. Video credit is granted
    /// only after the server processes the acknowledgment.
    pub fn report_presented(&self, header: &DesktopFrameHeader) -> Result<()> {
        self.presentation_control().report_presented(header)
    }
    /// Same screen ID explicitly restarts a stalled video stream. A cancelled
    /// switch leaves video absent; there is no automatic replay or stale reuse.
    pub async fn select_screen(&mut self, screen_id: String) -> Result<()> {
        validate_desktop_screen_id(&screen_id).map_err(DesktopError::Protocol)?;
        let previous = self.opened.config.clone();
        self.generation_ended.send_replace(true);
        *self.presentation_valid.lock().unwrap() = false;
        self.presentations.send_replace(None);
        self.last_received = Arc::new(AtomicU64::new(0));
        self.presentation_valid = Arc::new(Mutex::new(true));
        self.video = None;
        self.video_keepalive = None;
        let opened = match self
            .command(DesktopControlCommand::SelectScreen {
                generation: previous.generation,
                screen_id: screen_id.clone(),
            })
            .await?
        {
            DesktopControlReply::Configured { opened, .. } => opened,
            _ => return Err(DesktopError::Protocol("unexpected screen switch reply")),
        };
        opened.validate().map_err(DesktopError::Protocol)?;
        if opened.config.session_id != previous.session_id
            || Some(opened.config.generation) != previous.generation.checked_add(1)
            || opened.config.screen.id != screen_id
            || opened.config.fps != previous.fps
        {
            return Err(DesktopError::Protocol("invalid screen switch generation"));
        }
        self.opened = opened;
        self.generation_ended = watch::channel(false).0;
        self.attach_video().await
    }
    pub async fn close(&mut self) -> Result<()> {
        if self.close_confirmed {
            return Ok(());
        }
        self.closed = true;
        self.generation_ended.send_replace(true);
        *self.presentation_valid.lock().unwrap() = false;
        self.video = None;
        self.video_keepalive = None;
        self.worker.abort();
        close_id(
            &self.connection,
            self.opened.config.session_id,
            self.timeout,
        )
        .await?;
        self.close_confirmed = true;
        Ok(())
    }
}
impl Drop for RemoteDesktopSession {
    fn drop(&mut self) {
        self.generation_ended.send_replace(true);
        *self.presentation_valid.lock().unwrap() = false;
        self.worker.abort();
        if !self.close_confirmed {
            drop(OpenCleanup {
                connection: self.connection.clone(),
                id: self.opened.config.session_id,
                timeout: self.timeout,
                armed: true,
            });
        }
    }
}
async fn control_worker(
    stream: PabBiStream,
    mut commands: mpsc::Receiver<Command>,
    mut presentations: watch::Receiver<Option<(u64, u64)>>,
    heartbeat: Duration,
    timeout: Duration,
) {
    let (mut writer, mut reader) = stream.into_framed();
    let mut ticks = tokio::time::interval(heartbeat);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut sequence = 0_u64;
    loop {
        let (command, response) = tokio::select! {
            value = commands.recv() => match value {
                Some(value) => (value.command, Some(value.result)), None => break,
            },
            changed = presentations.changed() => {
                if changed.is_err() { break; }
                let latest = *presentations.borrow_and_update();
                let Some((generation, frame_id)) = latest else { continue };
                (DesktopControlCommand::Presented { generation, frame_id }, None)
            },
            _ = ticks.tick() => (DesktopControlCommand::Heartbeat, None),
        };
        let Some(next) = sequence.checked_add(1) else {
            break;
        };
        sequence = next;
        let request = DesktopControl { sequence, command };
        let operation = async {
            writer
                .write(&serde_json::to_vec(&request)?, DESKTOP_CONTROL_BYTES)
                .await?;
            let reply: DesktopControlReply =
                serde_json::from_slice(&reader.read(DESKTOP_CONTROL_BYTES).await?)?;
            if reply.sequence() != sequence {
                return Err(DesktopError::Protocol(
                    "desktop control reply sequence mismatch",
                ));
            }
            match reply {
                DesktopControlReply::Error { code, message, .. } => {
                    Err(DesktopError::reject(code, message))
                }
                other => Ok(other),
            }
        };
        let result = match tokio::time::timeout(timeout, operation).await {
            Ok(result) => result,
            Err(_) => Err(DesktopError::Timeout),
        };
        let fatal = result.as_ref().is_err_and(|error| {
            response.is_none() || !matches!(error, DesktopError::Rejected { .. })
        });
        if let Some(response) = response {
            let _ = response.send(result);
        }
        if fatal {
            break;
        } // Never retry a partially read control message.
    }
}
