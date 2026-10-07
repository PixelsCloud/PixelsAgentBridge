use std::{sync::Arc, time::Duration};

use pab_protocol::{DeviceTaskRequest, DeviceTaskResponse, OperatorRef, RequestId};
use pab_terminal::{MAX_INPUT_BYTES, MAX_READ_BYTES, TerminalSession};
use pab_transport::PabBiStream;
use tokio::sync::Mutex;

use super::{MAX_ACTIVE_TERMINALS, TaskService, TaskServiceError};
#[cfg(test)]
mod tests;

enum Session {
    Service(Arc<TerminalSession>),
    User(crate::user_worker::terminal::UserTerminal),
}
impl Session {
    async fn input(&self, bytes: &[u8]) -> Result<(), TaskServiceError> {
        match self {
            Self::Service(session) => {
                let session = session.clone();
                let bytes = bytes.to_vec();
                tokio::task::spawn_blocking(move || session.input(&bytes)).await??;
            }
            Self::User(session) => session.input(bytes).await?,
        };
        Ok(())
    }
    async fn read(
        &self,
        offset: u64,
        limit: usize,
    ) -> Result<pab_terminal::TerminalOutput, TaskServiceError> {
        Ok(match self {
            Self::Service(session) => session.read(offset, limit),
            Self::User(session) => session.read(offset, limit).await?,
        })
    }
    async fn resize(&self, cols: u16, rows: u16) -> Result<(), TaskServiceError> {
        match self {
            Self::Service(session) => {
                let session = session.clone();
                tokio::task::spawn_blocking(move || session.resize(cols, rows)).await??;
            }
            Self::User(session) => session.resize(cols, rows).await?,
        };
        Ok(())
    }
    async fn close(&self) -> Result<(), TaskServiceError> {
        match self {
            Self::Service(session) => {
                let session = session.clone();
                tokio::task::spawn_blocking(move || session.close()).await??;
            }
            Self::User(session) => session.close().await?,
        };
        Ok(())
    }
}

// Separate from task clones: dropping a network session closes its terminals
// even if accepted commands still keep a TaskService clone alive.
pub(crate) struct TerminalConnectionGuard(TaskService);
impl Drop for TerminalConnectionGuard {
    fn drop(&mut self) {
        self.0
            .ui_connection
            .closed
            .store(true, std::sync::atomic::Ordering::Release);
        let service = self.0.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                service.close_connection_terminals().await;
            });
        }
    }
}

pub(super) struct ActiveTerminal {
    owner: OperatorRef,
    connection: RequestId,
    selection: pab_protocol::ExecutionSelection,
    identity: Option<pab_protocol::ExecutionIdentity>,
    shell: String,
    startup: pab_protocol::TerminalStartup,
    cols: u16,
    rows: u16,
    session: Arc<Session>,
    next_sequence: Mutex<u64>,
    closed: std::sync::atomic::AtomicBool,
}

impl TaskService {
    async fn finish_terminal_action(
        &self,
        id: RequestId,
        sequence: u64,
        result: Result<(), TaskServiceError>,
    ) -> Result<(), TaskServiceError> {
        self.store
            .finish_terminal_event(
                id,
                sequence,
                if result.is_ok() {
                    "completed"
                } else {
                    "unconfirmed"
                },
            )
            .await?;
        if result.is_err() {
            self.store
                .finish_terminal(
                    id,
                    "interrupted",
                    Some("terminal operation result unconfirmed; input is not replayed"),
                )
                .await?;
            self.terminals.lock().await.remove(&id);
        }
        result
    }
    pub(crate) fn terminal_connection_guard(&self) -> TerminalConnectionGuard {
        TerminalConnectionGuard(self.clone())
    }

    async fn close_connection_terminals(&self) {
        let entries = {
            let mut sessions = self.terminals.lock().await;
            let ids = sessions
                .iter()
                .filter(|(_, entry)| entry.connection == self.ui_connection.id)
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| sessions.remove(&id).map(|entry| (id, entry)))
                .collect::<Vec<_>>()
        };
        for (id, entry) in entries {
            let _action = entry.next_sequence.lock().await;
            if entry.closed.load(std::sync::atomic::Ordering::Acquire) {
                continue;
            }
            let result = entry.session.close().await;
            let _ = self
                .store
                .finish_terminal(
                    id,
                    "interrupted",
                    Some(if result.is_ok() {
                        "connection ended; terminal closed"
                    } else {
                        "connection ended; terminal cleanup unconfirmed"
                    }),
                )
                .await;
        }
    }

    pub(super) async fn open_terminal_session(
        &self,
        owner: OperatorRef,
        id: RequestId,
        cols: u16,
        rows: u16,
        selection: pab_protocol::ExecutionSelection,
    ) -> Result<Arc<ActiveTerminal>, TaskServiceError> {
        if !(20..=500).contains(&cols) || !(5..=200).contains(&rows) {
            return Err(TaskServiceError::InvalidRequest(
                "invalid terminal dimensions",
            ));
        }
        let mut sessions = self.terminals.lock().await;
        if self
            .ui_connection
            .closed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(TaskServiceError::InvalidRequest("connection ended"));
        }
        if let Some(entry) = sessions.get(&id) {
            if entry.owner != owner || entry.connection != self.ui_connection.id {
                return Err(TaskServiceError::AccessDenied);
            }
            if entry.selection != selection || entry.cols != cols || entry.rows != rows {
                return Err(TaskServiceError::Store(
                    crate::task_store::TaskStoreError::RequestConflict,
                ));
            }
            return Ok(entry.clone());
        }
        if sessions.len() >= MAX_ACTIVE_TERMINALS {
            return Err(TaskServiceError::InvalidRequest(
                "too many terminal sessions",
            ));
        }
        let prepared = self.prepare_user(owner, selection).await?;
        let identity = prepared
            .as_ref()
            .map(|(_, identity)| identity.clone())
            .or_else(|| self.execution_context.identity.clone());
        let (shell, args) = crate::user_worker::terminal::shell();
        self.store
            .start_terminal(
                id,
                owner,
                shell,
                self.ui_connection.id,
                selection,
                identity.as_ref(),
                cols,
                rows,
            )
            .await?;
        let started: Result<Session, TaskServiceError> = async {
            Ok(if let Some((prepared, _)) = prepared {
                Session::User(
                    crate::user_worker::terminal::UserTerminal::start(
                        &self.user_worker_executable()?,
                        prepared,
                        cols,
                        rows,
                    )
                    .await?,
                )
            } else {
                Session::Service(Arc::new(
                    tokio::task::spawn_blocking(move || {
                        TerminalSession::start(shell, args, cols, rows)
                    })
                    .await??,
                ))
            })
        }
        .await;
        let session = match started {
            Ok(session) => session,
            Err(error) => {
                self.store
                    .finish_terminal(id, "failed", Some("terminal start failed"))
                    .await?;
                return Err(error);
            }
        };
        if self
            .ui_connection
            .closed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            let _ = session.close().await;
            self.store
                .finish_terminal(
                    id,
                    "interrupted",
                    Some("connection ended during terminal start"),
                )
                .await?;
            return Err(TaskServiceError::InvalidRequest("connection ended"));
        }
        let entry = Arc::new(ActiveTerminal {
            owner,
            connection: self.ui_connection.id,
            selection,
            identity,
            shell: shell.to_owned(),
            startup: crate::user_worker::terminal::startup(),
            cols,
            rows,
            session: Arc::new(session),
            next_sequence: Mutex::new(1),
            closed: std::sync::atomic::AtomicBool::new(false),
        });
        sessions.insert(id, entry.clone());
        Ok(entry)
    }

    pub(super) async fn handle_terminal(
        &self,
        initiated_by: OperatorRef,
        request: DeviceTaskRequest,
        stream: &mut PabBiStream,
        timeout: Duration,
    ) -> Result<(), TaskServiceError> {
        match request {
            DeviceTaskRequest::OpenTerminal {
                request_id,
                cols,
                rows,
                execution,
                ..
            } => {
                let entry = self
                    .open_terminal_session(initiated_by, request_id, cols, rows, execution)
                    .await?;
                stream
                    .send_json(
                        &DeviceTaskResponse::TerminalOpened {
                            session_id: request_id,
                            shell: entry.shell.clone(),
                            startup: Some(entry.startup.clone()),
                            cols,
                            rows,
                            identity: entry.identity.clone(),
                        },
                        timeout,
                    )
                    .await?;
            }
            DeviceTaskRequest::TerminalInput {
                session_id,
                sequence,
                size,
                ..
            } => {
                if size == 0 || size as usize > MAX_INPUT_BYTES {
                    return Err(TaskServiceError::InvalidRequest(
                        "invalid terminal input size",
                    ));
                }
                let entry = self.terminal_entry(session_id, initiated_by).await?;
                let bytes = stream.receive_binary_frame(timeout).await?;
                if bytes.len() != size as usize {
                    return Err(TaskServiceError::InvalidRequest(
                        "terminal input size mismatch",
                    ));
                }
                let mut next = entry.next_sequence.lock().await;
                if sequence != *next {
                    return Err(TaskServiceError::InvalidRequest(
                        "terminal input sequence mismatch",
                    ));
                }
                self.store
                    .begin_terminal_event(session_id, sequence, "input", &bytes)
                    .await?;
                *next += 1;
                let session = Arc::clone(&entry.session);
                let result = session.input(&bytes).await;
                self.finish_terminal_action(session_id, sequence, result)
                    .await?;
                stream
                    .send_json(
                        &DeviceTaskResponse::TerminalAcknowledged {
                            session_id,
                            sequence,
                        },
                        timeout,
                    )
                    .await?;
            }
            DeviceTaskRequest::TerminalRead {
                session_id,
                offset,
                limit,
                ..
            } => {
                if limit == 0 || limit as usize > MAX_READ_BYTES {
                    return Err(TaskServiceError::InvalidRequest(
                        "invalid terminal read size",
                    ));
                }
                let entry = self.terminal_entry(session_id, initiated_by).await?;
                // Serialize reads with close so a user-worker IPC read cannot
                // race the handoff to the retained final output.
                let _action = entry.next_sequence.lock().await;
                let output = match entry.session.read(offset, limit as usize).await {
                    Ok(output) => output,
                    Err(error) => {
                        self.store
                            .finish_terminal(
                                session_id,
                                "interrupted",
                                Some("terminal output unavailable; session is not reopened"),
                            )
                            .await?;
                        self.terminals.lock().await.remove(&session_id);
                        return Err(error);
                    }
                };
                let size = output.bytes.len() as u16;
                let response = DeviceTaskResponse::TerminalOutput {
                    session_id,
                    retained_from: output.retained_from,
                    offset: output.offset,
                    next_offset: output.next_offset,
                    size,
                    ended: output.ended,
                };
                if size == 0 {
                    stream.send_json(&response, timeout).await?;
                } else {
                    stream.send_frame_json(&response, timeout).await?;
                    stream.send_binary_frame(&output.bytes, timeout).await?;
                    stream.finish_send(timeout).await?;
                }
                if output.ended {
                    entry.session.close().await?;
                    self.store
                        .finish_terminal(session_id, "closed", None)
                        .await?;
                    entry
                        .closed
                        .store(true, std::sync::atomic::Ordering::Release);
                    self.terminals.lock().await.remove(&session_id);
                }
            }
            DeviceTaskRequest::TerminalResize {
                session_id,
                sequence,
                cols,
                rows,
                ..
            } => {
                let entry = self.terminal_entry(session_id, initiated_by).await?;
                let mut next = entry.next_sequence.lock().await;
                if sequence != *next {
                    return Err(TaskServiceError::InvalidRequest(
                        "terminal resize sequence mismatch",
                    ));
                }
                let payload = format!("{cols}x{rows}");
                self.store
                    .begin_terminal_event(session_id, sequence, "resize", payload.as_bytes())
                    .await?;
                *next += 1;
                let session = Arc::clone(&entry.session);
                let result = session.resize(cols, rows).await;
                self.finish_terminal_action(session_id, sequence, result)
                    .await?;
                stream
                    .send_json(
                        &DeviceTaskResponse::TerminalAcknowledged {
                            session_id,
                            sequence,
                        },
                        timeout,
                    )
                    .await?;
            }
            DeviceTaskRequest::TerminalClose {
                session_id,
                sequence,
                ..
            } => {
                let entry = self.terminal_entry(session_id, initiated_by).await?;
                let mut next = entry.next_sequence.lock().await;
                if sequence != *next {
                    return Err(TaskServiceError::InvalidRequest(
                        "terminal close sequence mismatch",
                    ));
                }
                self.store
                    .begin_terminal_event(session_id, sequence, "close", &[])
                    .await?;
                *next += 1;
                let session = Arc::clone(&entry.session);
                let result = session.close().await;
                self.finish_terminal_action(session_id, sequence, result)
                    .await?;
                self.store
                    .finish_terminal(session_id, "closed", None)
                    .await?;
                entry
                    .closed
                    .store(true, std::sync::atomic::Ordering::Release);
                // Keep the bounded output until EOF is read. A client that
                // disappears without draining cannot retain a slot forever.
                let sessions = Arc::downgrade(&self.terminals);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    if let Some(sessions) = sessions.upgrade() {
                        sessions.lock().await.remove(&session_id);
                    }
                });
                stream
                    .send_json(&DeviceTaskResponse::TerminalClosed { session_id }, timeout)
                    .await?;
            }
            _ => unreachable!("only terminal requests enter this handler"),
        }
        Ok(())
    }

    async fn terminal_entry(
        &self,
        id: RequestId,
        initiated_by: OperatorRef,
    ) -> Result<Arc<ActiveTerminal>, TaskServiceError> {
        let entry = self
            .terminals
            .lock()
            .await
            .get(&id)
            .cloned()
            .ok_or(TaskServiceError::NotFound)?;
        if entry.owner != initiated_by || entry.connection != self.ui_connection.id {
            return Err(TaskServiceError::AccessDenied);
        }
        Ok(entry)
    }
}
