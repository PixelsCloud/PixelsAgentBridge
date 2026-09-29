use std::{sync::Arc, time::Duration};

use pab_protocol::{DeviceTaskRequest, DeviceTaskResponse, OperatorRef, RequestId};
use pab_terminal::{MAX_INPUT_BYTES, MAX_READ_BYTES, TerminalSession};
use pab_transport::PabBiStream;
use tokio::sync::Mutex;

use super::{MAX_ACTIVE_TERMINALS, TaskService, TaskServiceError};

pub(super) struct ActiveTerminal {
    owner: OperatorRef,
    session: Arc<TerminalSession>,
    next_sequence: Mutex<u64>,
}

impl TaskService {
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
                ..
            } => {
                if self.terminals.lock().await.len() >= MAX_ACTIVE_TERMINALS {
                    return Err(TaskServiceError::InvalidRequest(
                        "too many terminal sessions",
                    ));
                }
                let (shell, args): (&str, &[&str]) = if cfg!(target_os = "windows") {
                    ("powershell.exe", &["-NoLogo", "-NoProfile"])
                } else {
                    ("/bin/sh", &["-i"])
                };
                self.store
                    .start_terminal(request_id, initiated_by, shell)
                    .await?;
                let started = tokio::task::spawn_blocking(move || {
                    TerminalSession::start(shell, args, cols, rows)
                })
                .await?;
                let session = match started {
                    Ok(session) => Arc::new(session),
                    Err(error) => {
                        self.store
                            .finish_terminal(request_id, "failed", Some(&error.to_string()))
                            .await?;
                        return Err(error.into());
                    }
                };
                self.terminals.lock().await.insert(
                    request_id,
                    Arc::new(ActiveTerminal {
                        owner: initiated_by,
                        session,
                        next_sequence: Mutex::new(1),
                    }),
                );
                stream
                    .send_json(
                        &DeviceTaskResponse::TerminalOpened {
                            session_id: request_id,
                            shell: shell.to_owned(),
                            cols,
                            rows,
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
                let result = tokio::task::spawn_blocking(move || session.input(&bytes)).await?;
                self.store
                    .finish_terminal_event(
                        session_id,
                        sequence,
                        if result.is_ok() {
                            "completed"
                        } else {
                            "failed"
                        },
                    )
                    .await?;
                result?;
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
                let output = entry.session.read(offset, limit as usize);
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
                    self.store
                        .finish_terminal(session_id, "closed", None)
                        .await?;
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
                let result =
                    tokio::task::spawn_blocking(move || session.resize(cols, rows)).await?;
                self.store
                    .finish_terminal_event(
                        session_id,
                        sequence,
                        if result.is_ok() {
                            "completed"
                        } else {
                            "failed"
                        },
                    )
                    .await?;
                result?;
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
                let result = tokio::task::spawn_blocking(move || session.close()).await?;
                self.store
                    .finish_terminal_event(
                        session_id,
                        sequence,
                        if result.is_ok() {
                            "completed"
                        } else {
                            "failed"
                        },
                    )
                    .await?;
                result?;
                self.store
                    .finish_terminal(session_id, "closed", None)
                    .await?;
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
        if entry.owner != initiated_by {
            return Err(TaskServiceError::AccessDenied);
        }
        Ok(entry)
    }
}
