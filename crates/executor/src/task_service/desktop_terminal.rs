use super::*;
use crate::user_worker::{
    self,
    desktop_terminal::{WorkerControl, WorkerOutput},
};
use pab_protocol::{
    DESKTOP_TERMINAL_CHUNK, DESKTOP_TERMINAL_SCHEMA_VERSION, DesktopTerminalControl,
    DesktopTerminalCredit, DesktopTerminalOpened, DesktopTerminalReply, DesktopTerminalRequest,
    ExecutionEnvironmentSource, ExecutionMode, RequestId,
};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(super) struct DesktopTerminals {
    sessions: Arc<Mutex<HashMap<RequestId, Arc<Entry>>>>,
    global: Arc<Semaphore>,
    connection: Arc<Semaphore>,
    reservations: Arc<std::sync::Mutex<std::collections::HashSet<RequestId>>>,
}
impl Default for DesktopTerminals {
    fn default() -> Self {
        Self {
            sessions: Default::default(),
            reservations: Default::default(),
            global: Arc::new(Semaphore::new(32)),
            connection: Arc::new(Semaphore::new(8)),
        }
    }
}
impl DesktopTerminals {
    pub(super) fn for_connection(&self) -> Self {
        Self {
            sessions: self.sessions.clone(),
            reservations: self.reservations.clone(),
            global: self.global.clone(),
            connection: Arc::new(Semaphore::new(8)),
        }
    }
}
struct Entry {
    owner: OperatorRef,
    account: pab_protocol::UserId,
    connection: RequestId,
    child: Arc<Mutex<pab_os_control::execution::UserProcess>>,
    control: Mutex<WorkerControl>,
    output: Mutex<Option<pab_os_control::execution::channel::WorkerStream>>,
    shutdown: Mutex<Option<tokio::io::WriteHalf<pab_os_control::execution::channel::WorkerStream>>>,
    control_attached: AtomicBool,
    closed: tokio::sync::watch::Sender<bool>,
    close_lock: Mutex<()>,
    _reservation: Reservation,
    _global: OwnedSemaphorePermit,
    _connection: OwnedSemaphorePermit,
}
struct Reservation {
    id: RequestId,
    ids: Arc<std::sync::Mutex<std::collections::HashSet<RequestId>>>,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if let Ok(mut ids) = self.ids.lock() {
            ids.remove(&self.id);
        }
    }
}
pub(crate) struct ConnectionGuard(TaskService);
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let service = self.0.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let entries: Vec<_> = service
                    .desktop_terminals
                    .sessions
                    .lock()
                    .await
                    .iter()
                    .filter(|(_, e)| e.connection == service.ui_connection.id)
                    .map(|(id, e)| (*id, e.clone()))
                    .collect();
                for (id, entry) in entries {
                    let _ = service.close_desktop_entry(id, entry).await;
                }
            });
        }
    }
}
impl TaskService {
    pub(crate) fn desktop_terminal_connection_guard(&self) -> ConnectionGuard {
        ConnectionGuard(self.clone())
    }
    async fn desktop_entry(
        &self,
        owner: OperatorRef,
        id: RequestId,
    ) -> Result<Arc<Entry>, TaskServiceError> {
        let entry = self
            .desktop_terminals
            .sessions
            .lock()
            .await
            .get(&id)
            .cloned()
            .ok_or(TaskServiceError::NotFound)?;
        if entry.owner != owner
            || entry.connection != self.ui_connection.id
            || self.user_context.user.as_ref().map(|u| u.user_id) != Some(entry.account)
        {
            return Err(TaskServiceError::AccessDenied);
        }
        Ok(entry)
    }
    async fn desktop_close_entry(
        &self,
        owner: OperatorRef,
        id: RequestId,
    ) -> Result<Arc<Entry>, TaskServiceError> {
        let entry = self
            .desktop_terminals
            .sessions
            .lock()
            .await
            .get(&id)
            .cloned()
            .ok_or(TaskServiceError::NotFound)?;
        // A logout/account switch must not strand owned workers. This permits
        // cleanup only, never input/output or adoption by the new account.
        if entry.owner != owner || entry.connection != self.ui_connection.id {
            return Err(TaskServiceError::AccessDenied);
        }
        Ok(entry)
    }
    pub(crate) fn recheck_desktop_terminal_account(&self, account: Option<pab_protocol::UserId>) {
        if self.desktop_terminals.global.available_permits() == 32 {
            return;
        }
        let service = self.clone();
        tokio::spawn(async move {
            let entries: Vec<_> = service
                .desktop_terminals
                .sessions
                .lock()
                .await
                .iter()
                .filter(|(_, e)| {
                    e.connection == service.ui_connection.id && Some(e.account) != account
                })
                .map(|(id, e)| (*id, e.clone()))
                .collect();
            for (id, entry) in entries {
                let _ = service.close_desktop_entry(id, entry).await;
            }
        });
    }
    async fn close_desktop_entry(
        &self,
        id: RequestId,
        entry: Arc<Entry>,
    ) -> Result<(), TaskServiceError> {
        let _close = entry.close_lock.lock().await;
        entry.closed.send_replace(true);
        if let Some(mut shutdown) = entry.shutdown.lock().await.take() {
            use tokio::io::AsyncWriteExt;
            let _ =
                tokio::time::timeout(Duration::from_millis(200), shutdown.write_all(&[1])).await;
            let until = tokio::time::Instant::now() + Duration::from_secs(3);
            while tokio::time::Instant::now() < until {
                if entry.child.lock().await.try_wait()?.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        let child = entry.child.clone();
        // Job/group termination has its own lock; never wait for the PTY writer.
        let result = tokio::task::spawn_blocking(move || {
            let mut child = child.blocking_lock();
            child.terminate()
        })
        .await?;
        result?;
        // Release quota only after native termination confirmation. Concurrent
        // closes remain idempotent and never remove another owner's session.
        self.desktop_terminals.sessions.lock().await.remove(&id);
        Ok(())
    }
    pub(super) async fn desktop_terminal_stream(
        &self,
        owner: OperatorRef,
        request: DesktopTerminalRequest,
        mut stream: PabBiStream,
        timeout: Duration,
    ) -> Result<(), TaskServiceError> {
        let preflight = async {
            // Endpoint ownership and signed-in user attribution are distinct in
            // Pixels. A logged-in Desktop may keep a Guest endpoint OperatorRef.
            // Require the server-verified user context, not an enum guess.
            if self.user_context.user.is_none()
                && !matches!(request, DesktopTerminalRequest::Close { .. })
            {
                return Err(TaskServiceError::AccessDenied);
            }
            match &request {
                DesktopTerminalRequest::Open {
                    version,
                    cols,
                    rows,
                    ..
                } => {
                    if *version != DESKTOP_TERMINAL_SCHEMA_VERSION {
                        return Err(TaskServiceError::InvalidRequest(
                            "unsupported Desktop terminal version",
                        ));
                    }
                    pab_protocol::validate_desktop_terminal_size(*cols, *rows)
                        .map_err(TaskServiceError::InvalidRequest)?;
                }
                DesktopTerminalRequest::Control { session_id }
                | DesktopTerminalRequest::Output { session_id } => {
                    self.desktop_entry(owner, *session_id).await?;
                }
                DesktopTerminalRequest::Close { session_id } => {
                    self.desktop_close_entry(owner, *session_id).await?;
                }
            }
            Ok::<(), TaskServiceError>(())
        }
        .await;
        if let Err(error) = preflight {
            stream.send_json(&error_response(&error), timeout).await?;
            return Ok(());
        }
        match request {
            DesktopTerminalRequest::Open {
                version,
                session_id,
                execution,
                cols,
                rows,
            } => {
                let opening = async {
                    if version != DESKTOP_TERMINAL_SCHEMA_VERSION {
                        return Err(TaskServiceError::InvalidRequest(
                            "unsupported Desktop terminal version",
                        ));
                    }
                    pab_protocol::validate_desktop_terminal_size(cols, rows)
                        .map_err(TaskServiceError::InvalidRequest)?;
                    if self.ui_connection.closed.load(Ordering::Acquire) {
                        return Err(TaskServiceError::AccessDenied);
                    }
                    let global = self
                        .desktop_terminals
                        .global
                        .clone()
                        .try_acquire_owned()
                        .map_err(|_| {
                            TaskServiceError::InvalidRequest(
                                "Desktop terminal global quota reached",
                            )
                        })?;
                    let connection = self
                        .desktop_terminals
                        .connection
                        .clone()
                        .try_acquire_owned()
                        .map_err(|_| {
                            TaskServiceError::InvalidRequest(
                                "Desktop terminal connection quota reached",
                            )
                        })?;
                    let reservation = {
                        let mut ids = self.desktop_terminals.reservations.lock().map_err(|_| {
                            TaskServiceError::InvalidRequest("terminal reservation lock poisoned")
                        })?;
                        if !ids.insert(session_id) {
                            return Err(TaskServiceError::InvalidRequest(
                                "duplicate Desktop terminal session",
                            ));
                        }
                        Reservation {
                            id: session_id,
                            ids: self.desktop_terminals.reservations.clone(),
                        }
                    };
                    let (prepared, identity) = match self.prepare_user(owner, execution).await? {
                        Some(value) => value,
                        None => {
                            let prepared = tokio::task::spawn_blocking(
                                pab_os_control::execution::PreparedUser::for_current_service,
                            )
                            .await??;
                            let identity = prepared.identity().observation(
                                ExecutionMode::Service,
                                ExecutionEnvironmentSource::ServiceProcess,
                            )?;
                            (prepared, identity)
                        }
                    };
                    let worker = user_worker::desktop_terminal::start(
                        &self.user_worker_executable()?,
                        prepared,
                        cols,
                        rows,
                    )
                    .await?;
                    let opened = DesktopTerminalOpened {
                        session_id,
                        shell: worker.shell,
                        arguments: worker.arguments,
                        cwd: worker.cwd,
                        identity,
                        cols,
                        rows,
                    };
                    let (closed, _) = watch::channel(false);
                    let account = self
                        .user_context
                        .user
                        .as_ref()
                        .ok_or(TaskServiceError::AccessDenied)?
                        .user_id;
                    let entry = Arc::new(Entry {
                        owner,
                        account,
                        connection: self.ui_connection.id,
                        child: Arc::new(Mutex::new(worker.child)),
                        control: Mutex::new(worker.control),
                        output: Mutex::new(Some(worker.output)),
                        shutdown: Mutex::new(None),
                        control_attached: AtomicBool::new(false),
                        closed,
                        close_lock: Mutex::new(()),
                        _reservation: reservation,
                        _global: global,
                        _connection: connection,
                    });
                    self.desktop_terminals
                        .sessions
                        .lock()
                        .await
                        .insert(session_id, entry.clone());
                    if self.ui_connection.closed.load(Ordering::Acquire) {
                        self.close_desktop_entry(session_id, entry).await?;
                        return Err(TaskServiceError::AccessDenied);
                    }
                    Ok::<_, TaskServiceError>((opened, entry))
                }
                .await;
                let (opened, entry) = match opening {
                    Ok(value) => value,
                    Err(error) => {
                        stream
                            .send_json(
                                &DeviceTaskResponse::DesktopTerminal {
                                    reply: DesktopTerminalReply::Error {
                                        message: error.to_string(),
                                    },
                                },
                                timeout,
                            )
                            .await?;
                        return Ok(());
                    }
                };
                let reply = stream
                    .send_json(
                        &DeviceTaskResponse::DesktopTerminal {
                            reply: DesktopTerminalReply::Opened { opened },
                        },
                        timeout,
                    )
                    .await;
                if let Err(error) = reply {
                    let _ = self.close_desktop_entry(session_id, entry).await;
                    return Err(error.into());
                }
                // An opened session must attach both directions. This is a setup
                // deadline, never an idle timeout on a running human Shell.
                let service = self.clone();
                let entry = Arc::downgrade(&entry);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    let Some(entry) = entry.upgrade() else {
                        return;
                    };
                    if entry.output.lock().await.is_some()
                        || !entry.control_attached.load(Ordering::Acquire)
                    {
                        let _ = service.close_desktop_entry(session_id, entry).await;
                    }
                });
            }
            DesktopTerminalRequest::Close { session_id } => {
                let entry = self.desktop_close_entry(owner, session_id).await?;
                self.close_desktop_entry(session_id, entry).await?;
                stream
                    .send_json(
                        &DeviceTaskResponse::DesktopTerminal {
                            reply: DesktopTerminalReply::Closed,
                        },
                        timeout,
                    )
                    .await?;
            }
            DesktopTerminalRequest::Control { session_id } => {
                let entry = self.desktop_entry(owner, session_id).await?;
                if entry.control_attached.swap(true, Ordering::AcqRel) {
                    return Err(TaskServiceError::InvalidRequest(
                        "terminal control already attached",
                    ));
                }
                stream
                    .send_frame_json(
                        &DeviceTaskResponse::DesktopTerminal {
                            reply: DesktopTerminalReply::Ready,
                        },
                        timeout,
                    )
                    .await?;
                let (mut writer, mut reader) = stream.into_framed();
                let mut cancel = entry.closed.subscribe();
                let result = async {
                    loop {
                        // Cancelled reads abandon this stream forever, never reuse
                        // a partially consumed frame. Idle reads have no timeout.
                        let bytes = tokio::select! { value = reader.read(24 * 1024) => value?, _ = cancel.wait_for(|v| *v) => return Ok::<(), TaskServiceError>(()) };
                        let request: DesktopTerminalControl = serde_json::from_slice(&bytes).map_err(|_| TaskServiceError::InvalidRequest("invalid terminal control frame"))?;
                        request.validate().map_err(TaskServiceError::InvalidRequest)?;
                        let mut peer = entry.control.lock().await;
                        let applied = tokio::select! { value = peer.apply(request) => value, _ = cancel.wait_for(|v| *v) => return Ok(()) };
                        let reply = match applied { Ok(()) => DesktopTerminalReply::Applied, Err(e) => return Err(e.into()) };
                        writer.write(&serde_json::to_vec(&reply).map_err(std::io::Error::other)?, 4096).await?;
                    }
                }.await;
                let _ = self.close_desktop_entry(session_id, entry).await;
                result?;
            }
            DesktopTerminalRequest::Output { session_id } => {
                let entry = self.desktop_entry(owner, session_id).await?;
                let output =
                    entry
                        .output
                        .lock()
                        .await
                        .take()
                        .ok_or(TaskServiceError::InvalidRequest(
                            "terminal output already attached",
                        ))?;
                let (mut output, shutdown) = tokio::io::split(output);
                *entry.shutdown.lock().await = Some(shutdown);
                stream
                    .send_frame_json(
                        &DeviceTaskResponse::DesktopTerminal {
                            reply: DesktopTerminalReply::Ready,
                        },
                        timeout,
                    )
                    .await?;
                let (mut writer, mut reader) = stream.into_framed();
                let (ack_tx, mut acks) = tokio::sync::mpsc::channel(4);
                let ack_reader = AbortReader(tokio::spawn(async move {
                    loop {
                        let value = reader.read(8).await;
                        if ack_tx.send(value).await.is_err() {
                            break;
                        }
                    }
                }));
                let mut cancel = entry.closed.subscribe();
                let mut credit = DesktopTerminalCredit::default();
                let result = async {
                    loop {
                        while credit.available() < DESKTOP_TERMINAL_CHUNK as u64 {
                            let value = tokio::select! { value = acks.recv() => value.ok_or(TaskServiceError::InvalidRequest("terminal ACK channel ended"))??, _ = cancel.wait_for(|v| *v) => return Ok::<(), TaskServiceError>(()) };
                            apply_ack(&mut credit, &value)?;
                        }
                        // Persistently read a complete worker frame; ACK waits
                        // cannot cancel or misalign this authenticated IPC stream.
                        let frame = tokio::select! { value = user_worker::desktop_terminal::read_output(&mut output) => value?, _ = cancel.wait_for(|v| *v) => return Ok(()) };
                        match frame {
                            WorkerOutput::Bytes(data) => {
                                let offset = credit.reserve(data.len()).map_err(TaskServiceError::InvalidRequest)?;
                                let mut bytes = Vec::with_capacity(data.len() + 9); bytes.push(0); bytes.extend_from_slice(&offset.to_be_bytes()); bytes.extend_from_slice(&data);
                                tokio::select! { result = writer.write(&bytes, DESKTOP_TERMINAL_CHUNK + 9) => result?, _ = cancel.wait_for(|v| *v) => return Ok(()) };
                            }
                            WorkerOutput::End(data) => {
                                while credit.available() != pab_protocol::DESKTOP_TERMINAL_CREDIT {
                                    let value = tokio::select! { value = acks.recv() => value.ok_or(TaskServiceError::InvalidRequest("terminal ACK channel ended"))??, _ = cancel.wait_for(|v| *v) => return Ok(()) }; apply_ack(&mut credit, &value)?;
                                }
                                let mut bytes = vec![1]; bytes.extend_from_slice(&credit.sent().to_be_bytes()); bytes.extend_from_slice(&data); writer.write(&bytes, 4105).await?; writer.finish()?; return Ok(());
                            }
                        }
                        while let Ok(value) = acks.try_recv() { apply_ack(&mut credit, &value?)?; }
                    }
                }.await;
                drop(ack_reader);
                let close = self.close_desktop_entry(session_id, entry).await;
                result?;
                close?;
            }
        }
        Ok(())
    }
}
struct AbortReader(tokio::task::JoinHandle<()>);
impl Drop for AbortReader {
    fn drop(&mut self) {
        self.0.abort();
    }
}
fn apply_ack(credit: &mut DesktopTerminalCredit, bytes: &[u8]) -> Result<(), TaskServiceError> {
    let bytes: [u8; 8] = bytes
        .try_into()
        .map_err(|_| TaskServiceError::InvalidRequest("invalid terminal ACK frame"))?;
    credit
        .consume(u64::from_be_bytes(bytes))
        .map_err(TaskServiceError::InvalidRequest)
}
