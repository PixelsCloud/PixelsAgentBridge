use crate::{DesktopError, DesktopVideoWriter, Result, VideoFrame};
use pab_protocol::*;
use pab_transport::PabBiStream;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

const HEARTBEAT_MS: u32 = 5000;
const LEASE_MS: u32 = 15000;
const MEDIA_TIMEOUT: Duration = Duration::from_secs(5);
pub type MediaFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

pub struct MediaFrame {
    pub bytes: Vec<u8>,
    pub capture_time_us: u64,
    pub keyframe: bool,
}
/// An adapter must bound its own capture queue and stop/release its worker when
/// dropped, including cancellation while next_frame is pending. No archives.
pub trait DesktopSource: Send {
    fn next_frame(&mut self) -> MediaFuture<'_, MediaFrame>;
}
/// Owns only this viewer's held keys/buttons. Drop must release them, including
/// when apply is cancelled. Never replay events after an uncertain result.
pub trait DesktopInputSink: Send {
    fn apply(&mut self, event: RemoteDesktopInput) -> MediaFuture<'_, ()>;
}
pub trait DesktopBackend: Send + Sync {
    fn screens(&self) -> MediaFuture<'_, Vec<DesktopScreen>>;
    /// First frame must initialize H.264 decoding (IDR + SPS/PPS). One source per
    /// generation; no native capture starts before the video stream is attached.
    fn start(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopSource>>;
    fn supports_input(&self) -> bool {
        false
    }
    fn input(&self, _config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopInputSink>> {
        Box::pin(async {
            Err(DesktopError::reject(
                DesktopErrorCode::Unsupported,
                "remote desktop input is unavailable",
            ))
        })
    }
}
struct InputLease {
    generation: Arc<Generation>,
    sink: Box<dyn DesktopInputSink>,
}

struct Shared {
    backend: Option<Arc<dyn DesktopBackend>>,
    slot: Mutex<Option<Arc<Entry>>>,
}
struct Connection {
    id: RequestId,
    closed: AtomicBool,
}
#[derive(Clone)]
pub struct DesktopHost {
    shared: Arc<Shared>,
    connection: Arc<Connection>,
}
struct Entry {
    id: RequestId,
    owner: OperatorRef,
    account: Option<UserId>,
    connection: RequestId,
    state: Mutex<Option<Ready>>,
    cancelled: watch::Sender<bool>,
    last_seen: Mutex<Instant>,
    control_attached: AtomicBool,
}
struct Ready {
    video: Arc<Generation>,
}
struct Generation {
    config: DesktopVideoConfig,
    cancelled: watch::Sender<bool>,
    stopped: watch::Sender<bool>,
    claimed: AtomicBool,
    frame_ready: AtomicBool,
    progress: Mutex<(u64, u64)>, // sent, cumulatively presented
    credit: Notify,
}
fn signal() -> watch::Sender<bool> {
    watch::channel(false).0
}
async fn cancelled(sender: &watch::Sender<bool>) {
    let mut state = sender.subscribe();
    loop {
        if *state.borrow_and_update() {
            return;
        }
        if state.changed().await.is_err() {
            return;
        }
    }
}
impl Generation {
    fn new(config: DesktopVideoConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            cancelled: signal(),
            stopped: signal(),
            claimed: AtomicBool::new(false),
            frame_ready: AtomicBool::new(false),
            progress: Mutex::new((0, 0)),
            credit: Notify::new(),
        })
    }
    fn stop(&self) {
        self.cancelled.send_replace(true);
        if !self.claimed.load(Ordering::SeqCst) {
            self.stopped.send_replace(true);
        }
    }
    fn acknowledge(&self, id: u64) -> Result<()> {
        let mut progress = self.progress.lock().unwrap();
        if id < progress.1 || id > progress.0 || id == 0 {
            return Err(DesktopError::reject(
                DesktopErrorCode::InvalidRequest,
                "invalid presentation acknowledgment",
            ));
        }
        progress.1 = id;
        self.credit.notify_one();
        Ok(())
    }
    async fn next_id(&self) -> Result<u64> {
        loop {
            let available = self.credit.notified();
            {
                let mut progress = self.progress.lock().unwrap();
                if progress.0 - progress.1 < DESKTOP_MAX_IN_FLIGHT {
                    progress.0 = progress
                        .0
                        .checked_add(1)
                        .ok_or(DesktopError::Protocol("frame ID overflow"))?;
                    return Ok(progress.0);
                }
            }
            available.await;
        }
    }
}
impl Entry {
    fn stop(&self) {
        self.cancelled.send_replace(true);
        if let Some(state) = self.state.lock().unwrap().as_ref() {
            state.video.stop();
        }
    }
    fn video(&self, generation: u64) -> Result<Arc<Generation>> {
        let state = self.state.lock().unwrap();
        let ready = state.as_ref().ok_or(DesktopError::Closed)?;
        if ready.video.config.generation != generation {
            return Err(DesktopError::reject(
                DesktopErrorCode::StaleGeneration,
                "desktop generation changed",
            ));
        }
        Ok(ready.video.clone())
    }
}
impl Default for DesktopHost {
    fn default() -> Self {
        Self::new(None)
    }
}
impl DesktopHost {
    pub fn new(backend: Option<Arc<dyn DesktopBackend>>) -> Self {
        Self {
            shared: Arc::new(Shared {
                backend,
                slot: Mutex::new(None),
            }),
            connection: Arc::new(Connection {
                id: RequestId::new(),
                closed: AtomicBool::new(false),
            }),
        }
    }
    pub fn for_connection(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            connection: Arc::new(Connection {
                id: RequestId::new(),
                closed: AtomicBool::new(false),
            }),
        }
    }
    pub fn supported_version(&self) -> Option<u16> {
        self.shared
            .backend
            .as_ref()
            .map(|_| REMOTE_DESKTOP_SCHEMA_VERSION)
    }
    pub fn connection_guard(&self) -> DesktopConnectionGuard {
        DesktopConnectionGuard(self.clone())
    }
    pub fn recheck_account(&self, account: Option<UserId>) {
        let selected = self
            .shared
            .slot
            .lock()
            .unwrap()
            .as_ref()
            .filter(|e| e.connection == self.connection.id && e.account != account)
            .cloned();
        if let Some(entry) = selected {
            self.release(&entry);
        }
    }
    fn release(&self, entry: &Arc<Entry>) {
        entry.stop();
        // Occupancy is retained until the video task has dropped its native
        // source. A new viewer must not race the previous capture worker's exit.
        if entry.state.lock().unwrap().as_ref().is_some_and(|ready| {
            ready.video.claimed.load(Ordering::SeqCst) && !*ready.video.stopped.borrow()
        }) {
            return;
        }
        let mut slot = self.shared.slot.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, entry))
        {
            *slot = None;
        }
    }
    fn entry(
        &self,
        owner: OperatorRef,
        account: Option<UserId>,
        id: RequestId,
        closing: bool,
    ) -> Result<Arc<Entry>> {
        if self.connection.closed.load(Ordering::SeqCst) {
            return Err(DesktopError::Closed);
        }
        let slot = self.shared.slot.lock().unwrap();
        let entry = slot.as_ref().filter(|e| e.id == id).ok_or_else(|| {
            DesktopError::reject(DesktopErrorCode::NotFound, "desktop session not found")
        })?;
        if entry.owner != owner
            || entry.connection != self.connection.id
            || (!closing && entry.account != account)
        {
            return Err(DesktopError::reject(
                DesktopErrorCode::WrongOwner,
                "desktop session belongs to a different connection",
            ));
        }
        if *entry.cancelled.borrow() && !closing {
            return Err(DesktopError::Closed);
        }
        Ok(entry.clone())
    }
    async fn open(
        &self,
        owner: OperatorRef,
        account: Option<UserId>,
        version: u16,
        id: RequestId,
        screen_id: Option<String>,
        fps: u16,
    ) -> Result<(DesktopOpened, ReleaseGuard)> {
        if version != REMOTE_DESKTOP_SCHEMA_VERSION {
            return Err(DesktopError::reject(
                DesktopErrorCode::Unsupported,
                "unsupported remote desktop version",
            ));
        }
        if id.as_uuid().is_nil() || !(1..=60).contains(&fps) {
            return Err(DesktopError::Protocol("invalid desktop open parameters"));
        }
        if let Some(id) = &screen_id {
            validate_desktop_screen_id(id).map_err(DesktopError::Protocol)?;
        }
        let backend = self.shared.backend.as_ref().ok_or_else(|| {
            DesktopError::reject(
                DesktopErrorCode::Unsupported,
                "remote desktop media backend is not available",
            )
        })?;
        let entry = Arc::new(Entry {
            id,
            owner,
            account,
            connection: self.connection.id,
            state: Mutex::new(None),
            cancelled: signal(),
            last_seen: Mutex::new(Instant::now()),
            control_attached: AtomicBool::new(false),
        });
        {
            let mut slot = self.shared.slot.lock().unwrap();
            if self.connection.closed.load(Ordering::SeqCst) {
                return Err(DesktopError::Closed);
            }
            if slot.is_some() {
                return Err(DesktopError::reject(
                    DesktopErrorCode::Busy,
                    "this device already has a desktop viewer",
                ));
            }
            *slot = Some(entry.clone());
        }
        let guard = ReleaseGuard {
            host: self.clone(),
            entry: Some(entry.clone()),
        };
        let screens = tokio::select! {
            biased;
            _ = cancelled(&entry.cancelled) => return Err(DesktopError::Closed),
            value = tokio::time::timeout(MEDIA_TIMEOUT, backend.screens()) => value.map_err(|_| DesktopError::Timeout)??,
        };
        let opened = make_opened(
            id,
            1,
            screens,
            screen_id.as_deref(),
            fps,
            backend.supports_input(),
        )?;
        *entry.state.lock().unwrap() = Some(Ready {
            video: Generation::new(opened.config.clone()),
        });
        if *entry.cancelled.borrow() {
            return Err(DesktopError::Closed);
        }
        let host = self.clone();
        tokio::spawn(async move {
            loop {
                let deadline =
                    *entry.last_seen.lock().unwrap() + Duration::from_millis(LEASE_MS.into());
                tokio::select! {
                    _ = cancelled(&entry.cancelled) => break,
                    _ = tokio::time::sleep_until(deadline) => {
                        if Instant::now() >= *entry.last_seen.lock().unwrap() + Duration::from_millis(LEASE_MS.into()) { break; }
                    }
                }
            }
            host.release(&entry);
        });
        Ok((opened, guard))
    }
    async fn select_screen(
        &self,
        entry: &Arc<Entry>,
        generation: u64,
        screen_id: &str,
    ) -> Result<DesktopOpened> {
        validate_desktop_screen_id(screen_id).map_err(DesktopError::Protocol)?;
        let old = entry.video(generation)?;
        let next = generation
            .checked_add(1)
            .ok_or(DesktopError::Protocol("desktop generation overflow"))?;
        let backend = self.shared.backend.as_ref().ok_or(DesktopError::Closed)?;
        // A helper may currently be waiting indefinitely for a changed image.
        // Retire that source before asking the same helper for a fresh layout.
        // Otherwise a static desktop can deadlock display switching.
        old.stop();
        tokio::time::timeout(MEDIA_TIMEOUT, cancelled(&old.stopped))
            .await
            .map_err(|_| DesktopError::Timeout)?;
        let screens = tokio::time::timeout(MEDIA_TIMEOUT, backend.screens())
            .await
            .map_err(|_| DesktopError::Timeout)??;
        let opened = make_opened(
            entry.id,
            next,
            screens,
            Some(screen_id),
            old.config.fps,
            backend.supports_input(),
        )?;
        if *entry.cancelled.borrow() {
            return Err(DesktopError::Closed);
        }
        *entry.state.lock().unwrap() = Some(Ready {
            video: Generation::new(opened.config.clone()),
        });
        Ok(opened)
    }
    /// Called only after the existing device-session authentication has resolved
    /// the operator/account. They are not accepted from wire messages.
    pub async fn serve(
        &self,
        owner: OperatorRef,
        account: Option<UserId>,
        request: RemoteDesktopRequest,
        mut stream: PabBiStream,
        timeout: Duration,
    ) -> Result<()> {
        match request {
            RemoteDesktopRequest::Open {
                version,
                session_id,
                screen_id,
                fps,
            } => {
                match self
                    .open(owner, account, version, session_id, screen_id, fps)
                    .await
                {
                    Ok((opened, mut guard)) => {
                        reply(&mut stream, RemoteDesktopReply::Opened { opened }, timeout).await?;
                        guard.entry = None;
                    }
                    Err(error) => rejected(&mut stream, error, timeout).await?,
                }
            }
            RemoteDesktopRequest::Close { session_id } => {
                match self.entry(owner, account, session_id, true) {
                    Ok(entry) => {
                        self.release(&entry);
                        let video = entry
                            .state
                            .lock()
                            .unwrap()
                            .as_ref()
                            .map(|state| state.video.clone());
                        if let Some(video) = video {
                            tokio::time::timeout(MEDIA_TIMEOUT, cancelled(&video.stopped))
                                .await
                                .map_err(|_| DesktopError::Timeout)?;
                        }
                        reply(&mut stream, RemoteDesktopReply::Closed, timeout).await?;
                    }
                    Err(DesktopError::Rejected {
                        code: DesktopErrorCode::NotFound,
                        ..
                    }) => {
                        reply(&mut stream, RemoteDesktopReply::Closed, timeout).await?;
                    }
                    Err(error) => rejected(&mut stream, error, timeout).await?,
                }
            }
            RemoteDesktopRequest::Control { session_id } => {
                let entry = match self.entry(owner, account, session_id, false) {
                    Ok(entry) => entry,
                    Err(error) => return rejected(&mut stream, error, timeout).await,
                };
                if entry.control_attached.swap(true, Ordering::SeqCst) {
                    return rejected(
                        &mut stream,
                        DesktopError::reject(
                            DesktopErrorCode::Busy,
                            "desktop control already attached",
                        ),
                        timeout,
                    )
                    .await;
                }
                let _guard = ReleaseGuard {
                    host: self.clone(),
                    entry: Some(entry.clone()),
                };
                reply(&mut stream, RemoteDesktopReply::Ready, timeout).await?;
                tokio::select! {
                    biased;
                    _ = cancelled(&entry.cancelled) => {},
                    result = self.control(&entry, stream, timeout) => result?,
                }
            }
            RemoteDesktopRequest::Video {
                session_id,
                generation,
            } => {
                let validation = self
                    .entry(owner, account, session_id, false)
                    .and_then(|entry| {
                        if !entry.control_attached.load(Ordering::SeqCst) {
                            return Err(DesktopError::Protocol("attach control before video"));
                        }
                        let video = entry.video(generation)?;
                        if *video.cancelled.borrow() || video.claimed.swap(true, Ordering::SeqCst) {
                            return Err(DesktopError::reject(
                                DesktopErrorCode::StaleGeneration,
                                "video already attached or ended; restart its generation",
                            ));
                        }
                        Ok((entry, video))
                    });
                let (entry, video) = match validation {
                    Ok(value) => value,
                    Err(error) => return rejected(&mut stream, error, timeout).await,
                };
                let _guard = VideoGuard {
                    video: video.clone(),
                    host: self.clone(),
                    entry: entry.clone(),
                };
                tokio::select! {
                    biased;
                    _ = cancelled(&entry.cancelled) => {},
                    _ = cancelled(&video.cancelled) => {},
                    result = self.video(&video, stream, timeout) => result?,
                }
            }
        }
        Ok(())
    }
    async fn control(
        &self,
        entry: &Arc<Entry>,
        stream: PabBiStream,
        timeout: Duration,
    ) -> Result<()> {
        let (mut writer, mut reader) = stream.into_framed();
        let mut sequence = 0_u64;
        let mut input_lease: Option<InputLease> = None;
        loop {
            // No timeout/retry of a partial read. Lease expiry drops the entire
            // control future/stream via the outer cancellation select.
            let input: DesktopControl = {
                let read = reader.read(DESKTOP_CONTROL_BYTES);
                tokio::pin!(read);
                loop {
                    let active_generation =
                        input_lease.as_ref().map(|lease| lease.generation.clone());
                    let release = async move {
                        if let Some(generation) = active_generation {
                            cancelled(&generation.cancelled).await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    };
                    tokio::select! {
                        biased;
                        _ = release => input_lease = None,
                        message = &mut read => break serde_json::from_slice(&message?)?,
                    }
                }
            };
            sequence = sequence
                .checked_add(1)
                .ok_or(DesktopError::Protocol("control sequence overflow"))?;
            if input.sequence != sequence {
                return Err(DesktopError::Protocol("out-of-order desktop control"));
            }
            *entry.last_seen.lock().unwrap() = Instant::now();
            let action = match input.command {
                DesktopControlCommand::Heartbeat => Ok(None),
                DesktopControlCommand::Presented {
                    generation,
                    frame_id,
                } => entry
                    .video(generation)
                    .and_then(|v| v.acknowledge(frame_id))
                    .map(|_| None),
                DesktopControlCommand::SelectScreen {
                    generation,
                    screen_id,
                } => {
                    input_lease = None;
                    self.select_screen(entry, generation, &screen_id)
                        .await
                        .map(Some)
                }
                DesktopControlCommand::SetInput {
                    generation,
                    enabled,
                } => self
                    .set_input(entry, &mut input_lease, generation, enabled)
                    .await
                    .map(|_| None),
                DesktopControlCommand::Input { generation, event } => self
                    .apply_input(entry, &mut input_lease, generation, event)
                    .await
                    .map(|_| None),
            };
            let output = match action {
                Ok(None) => DesktopControlReply::Applied { sequence },
                Ok(Some(opened)) => DesktopControlReply::Configured { sequence, opened },
                Err(error) => DesktopControlReply::Error {
                    sequence,
                    code: error.code(),
                    message: error.to_string(),
                },
            };
            tokio::time::timeout(
                timeout,
                writer.write(&serde_json::to_vec(&output)?, DESKTOP_CONTROL_BYTES),
            )
            .await
            .map_err(|_| DesktopError::Timeout)??;
        }
    }
    async fn set_input(
        &self,
        entry: &Arc<Entry>,
        lease: &mut Option<InputLease>,
        generation: u64,
        enabled: bool,
    ) -> Result<()> {
        let video = entry.video(generation)?;
        if !enabled {
            *lease = None;
            return Ok(());
        }
        if *video.cancelled.borrow() || !video.frame_ready.load(Ordering::Acquire) {
            return Err(DesktopError::reject(
                DesktopErrorCode::MediaUnavailable,
                "wait for the current desktop image before enabling input",
            ));
        }
        if lease
            .as_ref()
            .is_some_and(|l| Arc::ptr_eq(&l.generation, &video))
        {
            return Ok(());
        }
        *lease = None;
        let backend = self.shared.backend.as_ref().ok_or(DesktopError::Closed)?;
        let sink = tokio::select! {
            biased;
            _ = cancelled(&video.cancelled) => return Err(DesktopError::Closed),
            result = tokio::time::timeout(MEDIA_TIMEOUT, backend.input(video.config.clone())) => result.map_err(|_| DesktopError::Timeout)??,
        };
        *lease = Some(InputLease {
            generation: video,
            sink,
        });
        Ok(())
    }
    async fn apply_input(
        &self,
        entry: &Arc<Entry>,
        lease: &mut Option<InputLease>,
        generation: u64,
        event: RemoteDesktopInput,
    ) -> Result<()> {
        event.validate().map_err(DesktopError::Protocol)?;
        let video = entry.video(generation)?;
        let active = lease
            .as_mut()
            .filter(|l| Arc::ptr_eq(&l.generation, &video))
            .ok_or_else(|| {
                DesktopError::reject(
                    DesktopErrorCode::InvalidRequest,
                    "remote desktop is view only",
                )
            })?;
        let result = tokio::select! {
            biased;
            _ = cancelled(&video.cancelled) => Err(DesktopError::Closed),
            result = tokio::time::timeout(MEDIA_TIMEOUT, active.sink.apply(event)) => result.unwrap_or(Err(DesktopError::Timeout)),
        };
        if result.is_err() {
            *lease = None;
        }
        result
    }
    async fn video(
        &self,
        video: &Arc<Generation>,
        mut stream: PabBiStream,
        timeout: Duration,
    ) -> Result<()> {
        let backend = self.shared.backend.as_ref().ok_or(DesktopError::Closed)?;
        let mut source =
            match tokio::time::timeout(MEDIA_TIMEOUT, backend.start(video.config.clone())).await {
                Ok(Ok(source)) => source,
                Ok(Err(error)) => return rejected(&mut stream, error, timeout).await,
                Err(_) => {
                    return rejected(
                        &mut stream,
                        DesktopError::reject(
                            DesktopErrorCode::MediaUnavailable,
                            "desktop capture startup timed out",
                        ),
                        timeout,
                    )
                    .await;
                }
            };
        reply(&mut stream, RemoteDesktopReply::Ready, timeout).await?;
        let (writer, mut reverse) = stream.into_framed();
        let mut writer = DesktopVideoWriter::new(writer, video.config.clone())?;
        // Reverse traffic is forbidden. EOF/STOP also ends capture promptly when
        // the viewer drops only its video stream, without waiting for heartbeat.
        let peer_end = reverse.read(1);
        tokio::pin!(peer_end);
        loop {
            let send = async {
                let frame_id = video.next_id().await?;
                let media = source.next_frame().await?;
                if media.bytes.is_empty() || media.bytes.len() > DESKTOP_MAX_FRAME_BYTES {
                    return Err(DesktopError::Protocol("media frame too large or empty"));
                }
                let frame = VideoFrame {
                    header: DesktopFrameHeader {
                        version: REMOTE_DESKTOP_SCHEMA_VERSION,
                        session_id: video.config.session_id,
                        generation: video.config.generation,
                        frame_id,
                        capture_time_us: media.capture_time_us,
                        keyframe: media.keyframe,
                        payload_bytes: media.bytes.len() as u32,
                    },
                    bytes: media.bytes,
                };
                writer.send(&frame, timeout).await?;
                video.frame_ready.store(true, Ordering::Release);
                Ok::<_, DesktopError>(())
            };
            tokio::select! { biased; _ = &mut peer_end => return Ok(()), result = send => result? }
        }
    }
}
fn make_opened(
    id: RequestId,
    generation: u64,
    screens: Vec<DesktopScreen>,
    selected: Option<&str>,
    fps: u16,
    input_supported: bool,
) -> Result<DesktopOpened> {
    validate_desktop_screens(&screens).map_err(DesktopError::Protocol)?;
    let screen = match selected {
        Some(id) => screens.iter().find(|s| s.id == id),
        None => screens
            .iter()
            .find(|s| s.primary)
            .or_else(|| screens.first()),
    }
    .ok_or_else(|| {
        DesktopError::reject(
            DesktopErrorCode::NotFound,
            "selected desktop screen no longer exists",
        )
    })?
    .clone();
    let opened = DesktopOpened {
        config: DesktopVideoConfig {
            session_id: id,
            generation,
            screen,
            codec: DesktopCodec::H264AnnexB,
            color: DesktopColor::Bt709Limited,
            fps,
        },
        screens,
        heartbeat_ms: HEARTBEAT_MS,
        lease_ms: LEASE_MS,
        input_supported,
    };
    opened.validate().map_err(DesktopError::Protocol)?;
    Ok(opened)
}
async fn reply(
    stream: &mut PabBiStream,
    reply: RemoteDesktopReply,
    timeout: Duration,
) -> Result<()> {
    stream
        .send_frame_json(&DeviceTaskResponse::RemoteDesktop { reply }, timeout)
        .await?;
    Ok(())
}
async fn rejected(stream: &mut PabBiStream, error: DesktopError, timeout: Duration) -> Result<()> {
    reply(
        stream,
        RemoteDesktopReply::Error {
            code: error.code(),
            message: error.to_string(),
        },
        timeout,
    )
    .await
}
struct ReleaseGuard {
    host: DesktopHost,
    entry: Option<Arc<Entry>>,
}
impl Drop for ReleaseGuard {
    fn drop(&mut self) {
        if let Some(entry) = &self.entry {
            self.host.release(entry);
        }
    }
}
struct VideoGuard {
    video: Arc<Generation>,
    host: DesktopHost,
    entry: Arc<Entry>,
}
impl Drop for VideoGuard {
    fn drop(&mut self) {
        self.video.cancelled.send_replace(true);
        self.video.stopped.send_replace(true);
        let closing = *self.entry.cancelled.borrow();
        if closing {
            self.host.release(&self.entry);
        }
    }
}
pub struct DesktopConnectionGuard(DesktopHost);
impl Drop for DesktopConnectionGuard {
    fn drop(&mut self) {
        self.0.connection.closed.store(true, Ordering::SeqCst);
        let selected = self
            .0
            .shared
            .slot
            .lock()
            .unwrap()
            .as_ref()
            .filter(|e| e.connection == self.0.connection.id)
            .cloned();
        if let Some(entry) = selected {
            self.0.release(&entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Backend {
        pending: bool,
    }
    impl DesktopBackend for Backend {
        fn screens(&self) -> MediaFuture<'_, Vec<DesktopScreen>> {
            Box::pin(async move {
                if self.pending {
                    std::future::pending::<()>().await;
                }
                Ok(vec![DesktopScreen {
                    id: "primary".into(),
                    name: "Main".into(),
                    x: 0,
                    y: 0,
                    desktop_width: 1920,
                    desktop_height: 1080,
                    pixel_width: 1920,
                    pixel_height: 1080,
                    scale_milli: 1000,
                    rotation: DesktopRotation::Identity,
                    primary: true,
                }])
            })
        }
        fn start(&self, _: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopSource>> {
            Box::pin(async { Err(DesktopError::Protocol("open must not start capture")) })
        }
    }
    fn owner() -> OperatorRef {
        OperatorRef::guest(EndpointKey::new([8; 32]))
    }

    struct PendingInput(Arc<AtomicBool>);
    impl Drop for PendingInput {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    impl DesktopInputSink for PendingInput {
        fn apply(&mut self, _: RemoteDesktopInput) -> MediaFuture<'_, ()> {
            Box::pin(async {
                self.0.store(true, Ordering::SeqCst);
                std::future::pending().await
            })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn input_timeout_releases_partially_applied_press_and_never_replays() {
        let host = DesktopHost::new(Some(Arc::new(Backend { pending: false })));
        let (_, guard) = host
            .open(
                owner(),
                None,
                REMOTE_DESKTOP_SCHEMA_VERSION,
                RequestId::new(),
                None,
                15,
            )
            .await
            .unwrap();
        let entry = guard.entry.as_ref().unwrap();
        let held = Arc::new(AtomicBool::new(false));
        let mut lease = Some(InputLease {
            generation: entry.video(1).unwrap(),
            sink: Box::new(PendingInput(held.clone())),
        });
        let event = RemoteDesktopInput::Key {
            virtual_key: 65,
            down: true,
        };
        assert!(matches!(
            host.apply_input(entry, &mut lease, 1, event.clone()).await,
            Err(DesktopError::Timeout)
        ));
        assert!(lease.is_none());
        assert!(!held.load(Ordering::SeqCst));
        assert!(host.apply_input(entry, &mut lease, 1, event).await.is_err());
    }

    #[tokio::test]
    async fn no_input_before_first_frame_or_after_generation_cancellation() {
        let host = DesktopHost::new(Some(Arc::new(Backend { pending: false })));
        let (_, guard) = host
            .open(
                owner(),
                None,
                REMOTE_DESKTOP_SCHEMA_VERSION,
                RequestId::new(),
                None,
                15,
            )
            .await
            .unwrap();
        let entry = guard.entry.as_ref().unwrap();
        let mut lease = None;
        assert!(matches!(
            host.set_input(entry, &mut lease, 1, true).await,
            Err(DesktopError::Rejected {
                code: DesktopErrorCode::MediaUnavailable,
                ..
            })
        ));
        let generation = entry.video(1).unwrap();
        generation.frame_ready.store(true, Ordering::Release);
        generation.stop();
        assert!(matches!(
            host.set_input(entry, &mut lease, 1, true).await,
            Err(DesktopError::Rejected {
                code: DesktopErrorCode::MediaUnavailable,
                ..
            })
        ));
        assert!(lease.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn abandoned_open_expires_and_old_guard_cannot_release_new_session() {
        let host = DesktopHost::new(Some(Arc::new(Backend { pending: false })));
        let (_, mut old_guard) = host
            .open(
                owner(),
                None,
                REMOTE_DESKTOP_SCHEMA_VERSION,
                RequestId::new(),
                None,
                15,
            )
            .await
            .unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(LEASE_MS.into())).await;
        tokio::task::yield_now().await;
        assert!(host.shared.slot.lock().unwrap().is_none());
        let (_, _new_guard) = host
            .open(
                owner(),
                None,
                REMOTE_DESKTOP_SCHEMA_VERSION,
                RequestId::new(),
                None,
                15,
            )
            .await
            .unwrap();
        let new_id = host.shared.slot.lock().unwrap().as_ref().unwrap().id;
        if let Some(old) = old_guard.entry.take() {
            host.release(&old);
        }
        assert_eq!(
            host.shared.slot.lock().unwrap().as_ref().unwrap().id,
            new_id
        );
    }
    #[tokio::test]
    async fn connection_guard_cancels_pending_open_and_blocks_late_publication() {
        let host = DesktopHost::new(Some(Arc::new(Backend { pending: true })));
        let connection = host.connection_guard();
        let opening = host.clone();
        let task = tokio::spawn(async move {
            opening
                .open(
                    owner(),
                    None,
                    REMOTE_DESKTOP_SCHEMA_VERSION,
                    RequestId::new(),
                    None,
                    15,
                )
                .await
        });
        while host.shared.slot.lock().unwrap().is_none() {
            tokio::task::yield_now().await;
        }
        drop(connection);
        assert!(matches!(task.await.unwrap(), Err(DesktopError::Closed)));
        assert!(host.shared.slot.lock().unwrap().is_none());
        assert!(
            host.open(
                owner(),
                None,
                REMOTE_DESKTOP_SCHEMA_VERSION,
                RequestId::new(),
                None,
                15
            )
            .await
            .is_err()
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_open_is_atomic_and_account_switch_releases_owner() {
        let host = DesktopHost::new(Some(Arc::new(Backend { pending: false })));
        let mut jobs = tokio::task::JoinSet::new();
        for _ in 0..20 {
            let contender = host.for_connection();
            jobs.spawn(async move {
                contender
                    .open(
                        owner(),
                        None,
                        REMOTE_DESKTOP_SCHEMA_VERSION,
                        RequestId::new(),
                        None,
                        15,
                    )
                    .await
            });
        }
        let mut winners = Vec::new();
        while let Some(result) = jobs.join_next().await {
            match result.unwrap() {
                Ok(value) => winners.push(value),
                Err(DesktopError::Rejected {
                    code: DesktopErrorCode::Busy,
                    ..
                }) => {}
                _ => panic!("unexpected open result"),
            }
        }
        assert_eq!(winners.len(), 1);
        let (_, guard) = winners.pop().unwrap();
        // Verified account context changing on the SAME authenticated connection
        // invalidates the old viewer. Other connections cannot trigger this.
        host.recheck_account(Some(UserId::new()));
        assert!(host.shared.slot.lock().unwrap().is_some());
        guard.host.recheck_account(Some(UserId::new()));
        assert!(host.shared.slot.lock().unwrap().is_none());
    }
}
