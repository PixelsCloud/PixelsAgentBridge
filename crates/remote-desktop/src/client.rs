use crate::{DesktopError, DesktopVideoReader, Result, VideoFrame};
use pab_protocol::*;
use pab_transport::{PabBiStream, PabConnection, PabFrameWriter};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};

struct Command {
    command: DesktopControlCommand,
    result: oneshot::Sender<Result<DesktopControlReply>>,
}
/// One viewer bound to one already-authenticated connection. Dropping it ends
/// its streams only. No reconnect, account changes, credential storage or input.
pub struct RemoteDesktopSession {
    connection: PabConnection,
    opened: DesktopOpened,
    timeout: Duration,
    commands: mpsc::Sender<Command>,
    worker: tokio::task::JoinHandle<()>,
    video: Option<DesktopVideoReader>,
    video_keepalive: Option<PabFrameWriter>,
    presentations: watch::Sender<Option<(u64, u64)>>,
    last_received: u64,
    closed: bool,
    close_confirmed: bool,
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
            last_received: 0,
            closed: false,
            close_confirmed: false,
        };
        session.attach_video().await?;
        cleanup.armed = false;
        Ok(session)
    }
    pub fn opened(&self) -> &DesktopOpened {
        &self.opened
    }
    pub fn connection_path(&self) -> Option<pab_transport::ConnectionPath> {
        self.connection.selected_path()
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
        self.last_received = frame.header.frame_id;
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
        self.last_received = frame.header.frame_id;
        Ok(frame)
    }
    /// Queue cumulative decode/display consumption without blocking the render
    /// loop on an RTT for each frame. Updates coalesce while control is in flight.
    /// This confirms local queuing, not remote delivery. Video credit is granted
    /// only after the server processes the acknowledgment.
    pub fn report_presented(&self, header: &DesktopFrameHeader) -> Result<()> {
        if self.closed || self.worker.is_finished() {
            return Err(DesktopError::Closed);
        }
        if header.session_id != self.opened.config.session_id
            || header.generation != self.opened.config.generation
            || header.frame_id == 0
            || header.frame_id > self.last_received
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
    /// Same screen ID explicitly restarts a stalled video stream. A cancelled
    /// switch leaves video absent; there is no automatic replay or stale reuse.
    pub async fn select_screen(&mut self, screen_id: String) -> Result<()> {
        validate_desktop_screen_id(&screen_id).map_err(DesktopError::Protocol)?;
        let previous = self.opened.config.clone();
        self.presentations.send_replace(None);
        self.last_received = 0;
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
        self.attach_video().await
    }
    pub async fn close(&mut self) -> Result<()> {
        if self.close_confirmed {
            return Ok(());
        }
        self.closed = true;
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
