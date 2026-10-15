use super::*;
use pab_protocol::{
    DESKTOP_TERMINAL_CHUNK, DESKTOP_TERMINAL_SCHEMA_VERSION, DesktopTerminalControl,
    DesktopTerminalEvent, DesktopTerminalOpened, DesktopTerminalReply, DesktopTerminalRequest,
    ExecutionSelection,
};
use pab_transport::{ConnectionPath, PabFrameReader, PabFrameWriter};
use sha2::{Digest, Sha256};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::sync::Mutex;

/// Pinned to one authenticated connection. It has no archive/database, retry,
/// implicit reconnection, or dependency on the stable MCP terminal machinery.
pub struct DesktopTerminalSession {
    pub opened: DesktopTerminalOpened,
    /// OS reported by the authenticated target, never inferred from its Shell.
    pub os_family: pab_protocol::OsFamily,
    connection: AuthenticatedDeviceConnection,
    control: Mutex<(PabFrameWriter, PabFrameReader, u64)>,
    output: Mutex<PabFrameReader>,
    ack: Mutex<(PabFrameWriter, u64)>,
    received: AtomicU64,
    digest: std::sync::Mutex<Sha256>,
    ended: AtomicBool,
    closed: AtomicBool,
    close_lock: Mutex<()>,
}
fn failed(message: impl Into<String>) -> BridgeError {
    BridgeError::Terminal(message.into())
}
fn reply(response: DeviceTaskResponse) -> Result<DesktopTerminalReply, BridgeError> {
    match response {
        DeviceTaskResponse::DesktopTerminal { reply } => Ok(reply),
        other => Err(unexpected_task_response(other)),
    }
}
impl AuthenticatedDeviceConnection {
    async fn desktop_stream(
        &self,
        request: DesktopTerminalRequest,
    ) -> Result<pab_transport::PabBiStream, BridgeError> {
        let mut stream = self.connection.open_bi(self.operation_timeout()).await?;
        stream
            .send_frame_json(
                &DeviceTaskRequest::DesktopTerminal {
                    schema_version: DEVICE_TASK_SCHEMA_VERSION,
                    request,
                },
                self.operation_timeout(),
            )
            .await?;
        Ok(stream)
    }
    pub async fn open_desktop_terminal(
        &self,
        execution: ExecutionSelection,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<DesktopTerminalSession>, BridgeError> {
        pab_protocol::validate_desktop_terminal_size(cols, rows).map_err(failed)?;
        let os_family = match self
            .task_request(DeviceTaskRequest::GetEnvironment {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
            })
            .await?
        {
            DeviceTaskResponse::Environment {
                context,
                desktop_terminal_schema_version: Some(DESKTOP_TERMINAL_SCHEMA_VERSION),
                ..
            } if context.device_ref == self.device_ref => context.execution.os_family,
            _ => {
                return Err(failed(
                    "this device needs an update to support Desktop remote terminals",
                ));
            }
        };
        let session_id = RequestId::new();
        let mut opening = self
            .desktop_stream(DesktopTerminalRequest::Open {
                version: DESKTOP_TERMINAL_SCHEMA_VERSION,
                session_id,
                execution,
                cols,
                rows,
            })
            .await?;
        let response = match opening.receive_json(self.operation_timeout()).await {
            Ok(response) => response,
            Err(error) => {
                let cleanup = self.close_desktop_terminal_id(session_id).await;
                return Err(failed(format!(
                    "Shell open result unconfirmed; it was not replayed; cleanup {}: {error}",
                    if cleanup.is_ok() {
                        "requested"
                    } else {
                        "unconfirmed"
                    }
                )));
            }
        };
        let opened = match reply(response)? {
            DesktopTerminalReply::Opened { opened }
                if opened.session_id == session_id
                    && opened.cols == cols
                    && opened.rows == rows
                    && opened.identity.mode == execution.mode()
                    && opened.identity.validate().is_ok() =>
            {
                opened
            }
            DesktopTerminalReply::Error { message } => return Err(failed(message)),
            other => {
                let _ = self.close_desktop_terminal_id(session_id).await;
                return Err(failed(format!(
                    "invalid Desktop terminal open reply: {other:?}"
                )));
            }
        };
        let channels = async {
            let mut control = self
                .desktop_stream(DesktopTerminalRequest::Control { session_id })
                .await?;
            if reply(control.receive_json(self.operation_timeout()).await?)?
                != DesktopTerminalReply::Ready
            {
                return Err(failed("terminal control stream rejected"));
            }
            let mut output = self
                .desktop_stream(DesktopTerminalRequest::Output { session_id })
                .await?;
            if reply(output.receive_json(self.operation_timeout()).await?)?
                != DesktopTerminalReply::Ready
            {
                return Err(failed("terminal output stream rejected"));
            }
            Ok::<_, BridgeError>((control.into_framed(), output.into_framed()))
        }
        .await;
        let ((control_writer, control_reader), (ack, output)) = match channels {
            Ok(v) => v,
            Err(e) => {
                let _ = self.close_desktop_terminal_id(session_id).await;
                return Err(e);
            }
        };
        Ok(Arc::new(DesktopTerminalSession {
            opened,
            os_family,
            connection: self.clone(),
            control: Mutex::new((control_writer, control_reader, 1)),
            output: Mutex::new(output),
            ack: Mutex::new((ack, 0)),
            received: AtomicU64::new(0),
            digest: std::sync::Mutex::new(Sha256::new()),
            ended: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            close_lock: Mutex::new(()),
        }))
    }
    pub(super) async fn close_desktop_terminal_id(
        &self,
        session_id: RequestId,
    ) -> Result<(), BridgeError> {
        match self
            .task_request(DeviceTaskRequest::DesktopTerminal {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request: DesktopTerminalRequest::Close { session_id },
            })
            .await
        {
            Ok(DeviceTaskResponse::DesktopTerminal {
                reply: DesktopTerminalReply::Closed,
            }) => Ok(()),
            // Natural exit has already removed the worker on the server.
            // task_request converts remote Error responses into BridgeError.
            Err(BridgeError::RemoteTask {
                code: DeviceTaskErrorCode::NotFound,
                ..
            }) => Ok(()),
            Ok(other) => Err(unexpected_task_response(other)),
            Err(error) => Err(error),
        }
    }
}
impl DesktopTerminalSession {
    pub fn connection_path(&self) -> Option<ConnectionPath> {
        self.connection.connection.selected_path()
    }
    pub async fn next_event(&self) -> Result<DesktopTerminalEvent, BridgeError> {
        if self.ended.load(Ordering::Acquire) {
            return Err(failed("terminal output has ended"));
        }
        let mut output = self.output.lock().await;
        let bytes = output.read(DESKTOP_TERMINAL_CHUNK + 9).await?;
        if bytes.len() < 9 {
            return Err(failed("incomplete Desktop terminal output header"));
        }
        let offset = u64::from_be_bytes(bytes[1..9].try_into().unwrap());
        if offset != self.received.load(Ordering::Acquire) {
            return Err(failed(
                "terminal output offset mismatch; no data will be silently skipped",
            ));
        }
        match bytes[0] {
            0 if bytes.len() > 9 => {
                self.digest
                    .lock()
                    .map_err(|_| failed("terminal digest lock poisoned"))?
                    .update(&bytes[9..]);
                self.received.store(
                    offset
                        .checked_add((bytes.len() - 9) as u64)
                        .ok_or_else(|| failed("terminal offset overflow"))?,
                    Ordering::Release,
                );
                Ok(DesktopTerminalEvent::Output {
                    offset,
                    bytes: bytes[9..].to_vec(),
                })
            }
            1 if bytes.len() <= 4105 => {
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct End {
                    exit_code: Option<u32>,
                    reason: String,
                    #[serde(default)]
                    output_bytes: Option<u64>,
                    #[serde(default)]
                    sha256: Option<String>,
                }
                let end: End =
                    serde_json::from_slice(&bytes[9..]).map_err(|e| failed(e.to_string()))?;
                if end.output_bytes.is_some_and(|n| n != offset) {
                    return Err(failed("PTY source and received byte counts differ"));
                }
                if let Some(hash) = &end.sha256 {
                    let actual = format!(
                        "{:x}",
                        self.digest
                            .lock()
                            .map_err(|_| failed("terminal digest lock poisoned"))?
                            .clone()
                            .finalize()
                    );
                    if &actual != hash {
                        return Err(failed("PTY source and received output hashes differ"));
                    }
                }
                self.ended.store(true, Ordering::Release);
                Ok(DesktopTerminalEvent::Ended {
                    offset,
                    exit_code: end.exit_code,
                    reason: end.reason,
                    output_bytes: end.output_bytes,
                    sha256: end.sha256,
                })
            }
            _ => Err(failed("invalid Desktop terminal output frame")),
        }
    }
    pub async fn consume(&self, offset: u64) -> Result<(), BridgeError> {
        let mut ack = self.ack.lock().await;
        if offset < ack.1 || offset > self.received.load(Ordering::Acquire) {
            return Err(failed("invalid renderer consumption offset"));
        }
        if offset == ack.1 {
            return Ok(());
        }
        ack.0.write(&offset.to_be_bytes(), 8).await?;
        ack.1 = offset;
        Ok(())
    }
    pub async fn input(&self, bytes: Vec<u8>) -> Result<(), BridgeError> {
        let mut channel = self.control.lock().await;
        let sequence = channel.2;
        let request = DesktopTerminalControl::Input { sequence, bytes };
        request.validate().map_err(failed)?;
        channel.2 = sequence
            .checked_add(1)
            .ok_or_else(|| failed("terminal input sequence overflow"))?;
        self.apply(&mut channel, request).await
    }
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), BridgeError> {
        let request = DesktopTerminalControl::Resize { cols, rows };
        request.validate().map_err(failed)?;
        let mut channel = self.control.lock().await;
        self.apply(&mut channel, request).await
    }
    async fn apply(
        &self,
        channel: &mut (PabFrameWriter, PabFrameReader, u64),
        request: DesktopTerminalControl,
    ) -> Result<(), BridgeError> {
        if self.closed.load(Ordering::Acquire) || self.ended.load(Ordering::Acquire) {
            return Err(failed("terminal is closed"));
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            channel
                .0
                .write(
                    &serde_json::to_vec(&request).map_err(|e| failed(e.to_string()))?,
                    24 * 1024,
                )
                .await?;
            let bytes = channel.1.read(4096).await?;
            match serde_json::from_slice::<DesktopTerminalReply>(&bytes)
                .map_err(|e| failed(e.to_string()))?
            {
                DesktopTerminalReply::Applied => Ok(()),
                DesktopTerminalReply::Error { message } => Err(failed(message)),
                _ => Err(failed("unexpected terminal control acknowledgement")),
            }
        })
        .await;
        match result {
            Ok(Ok(())) => Ok(()),
            error => {
                // A timed-out partial frame is never reused or replayed. End this
                // session through its independently reachable close stream.
                let _ = self.close().await;
                match error {
                    Ok(Err(e)) => Err(e),
                    _ => Err(failed(
                        "terminal input result unconfirmed; input was not replayed; reopen a new Shell",
                    )),
                }
            }
        }
    }
    pub async fn close(&self) -> Result<(), BridgeError> {
        let _guard = self.close_lock.lock().await;
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        self.connection
            .close_desktop_terminal_id(self.opened.session_id)
            .await?;
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}
impl Drop for DesktopTerminalSession {
    fn drop(&mut self) {
        if !self.closed.load(Ordering::Acquire) {
            let connection = self.connection.clone();
            let id = self.opened.session_id;
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _ = connection.close_desktop_terminal_id(id).await;
                });
            }
        }
    }
}
