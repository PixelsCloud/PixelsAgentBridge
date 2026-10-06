use pab_protocol::{
    DEVICE_TASK_SCHEMA_VERSION, DeviceTaskRequest, DeviceTaskResponse, MAX_TERMINAL_INPUT_BYTES,
    MAX_TERMINAL_READ_BYTES, RequestId,
};

use super::{AuthenticatedDeviceConnection, BridgeError, unexpected_task_response};

pub struct TerminalOpened {
    pub session_id: RequestId,
    pub shell: String,
    pub cols: u16,
    pub rows: u16,
    pub identity: Option<pab_protocol::ExecutionIdentity>,
}

pub struct TerminalOutput {
    pub retained_from: u64,
    pub offset: u64,
    pub next_offset: u64,
    pub bytes: Vec<u8>,
    pub ended: bool,
}

impl AuthenticatedDeviceConnection {
    pub async fn open_terminal(
        &self,
        session_id: RequestId,
        cols: u16,
        rows: u16,
    ) -> Result<TerminalOpened, BridgeError> {
        self.open_terminal_as(session_id, cols, rows, Default::default())
            .await
    }

    pub async fn open_terminal_as(
        &self,
        session_id: RequestId,
        cols: u16,
        rows: u16,
        execution: pab_protocol::ExecutionSelection,
    ) -> Result<TerminalOpened, BridgeError> {
        if !execution.is_service() {
            match self
                .task_request(DeviceTaskRequest::GetEnvironment {
                    schema_version: DEVICE_TASK_SCHEMA_VERSION,
                })
                .await?
            {
                DeviceTaskResponse::Environment {
                    context,
                    terminal_schema_version: Some(v),
                    ..
                } if context.device_ref == self.device_ref && v >= 2 => {}
                _ => {
                    return Err(BridgeError::Terminal(
                        "selected user requires terminal protocol v2".into(),
                    ));
                }
            }
        }
        let response = self
            .task_request(DeviceTaskRequest::OpenTerminal {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id: session_id,
                cols,
                rows,
                execution,
            })
            .await?;
        match response {
            DeviceTaskResponse::TerminalOpened {
                session_id: returned,
                shell,
                cols: returned_cols,
                rows: returned_rows,
                identity,
            } if returned == session_id && returned_cols == cols && returned_rows == rows => {
                if identity
                    .as_ref()
                    .is_some_and(|v| v.validate().is_err() || v.mode != execution.mode())
                    || (!execution.is_service() && identity.is_none())
                {
                    return Err(BridgeError::Terminal(
                        "terminal returned a missing or different execution identity".into(),
                    ));
                }
                Ok(TerminalOpened {
                    session_id,
                    shell,
                    cols,
                    rows,
                    identity,
                })
            }
            response => Err(unexpected_task_response(response)),
        }
    }

    pub async fn terminal_input(
        &self,
        session_id: RequestId,
        sequence: u64,
        bytes: &[u8],
    ) -> Result<(), BridgeError> {
        if bytes.is_empty() || bytes.len() > MAX_TERMINAL_INPUT_BYTES {
            return Err(BridgeError::Terminal(
                "terminal input size is invalid".to_owned(),
            ));
        }
        let mut stream = self.connection.open_bi(self.operation_timeout()).await?;
        stream
            .send_frame_json(
                &DeviceTaskRequest::TerminalInput {
                    schema_version: DEVICE_TASK_SCHEMA_VERSION,
                    session_id,
                    sequence,
                    size: bytes.len() as u16,
                },
                self.operation_timeout(),
            )
            .await?;
        stream
            .send_binary_frame(bytes, self.operation_timeout())
            .await?;
        stream.finish_send(self.operation_timeout()).await?;
        let response: DeviceTaskResponse = stream.receive_json(self.operation_timeout()).await?;
        match response {
            DeviceTaskResponse::TerminalAcknowledged {
                session_id: returned,
                sequence: returned_sequence,
            } if returned == session_id && returned_sequence == sequence => Ok(()),
            DeviceTaskResponse::Error { code, message } => {
                Err(BridgeError::RemoteTask { code, message })
            }
            response => Err(unexpected_task_response(response)),
        }
    }

    pub async fn terminal_read(
        &self,
        session_id: RequestId,
        offset: u64,
    ) -> Result<TerminalOutput, BridgeError> {
        let mut stream = self.connection.open_bi(self.operation_timeout()).await?;
        stream
            .send_json(
                &DeviceTaskRequest::TerminalRead {
                    schema_version: DEVICE_TASK_SCHEMA_VERSION,
                    session_id,
                    offset,
                    limit: MAX_TERMINAL_READ_BYTES as u16,
                },
                self.operation_timeout(),
            )
            .await?;
        let response: DeviceTaskResponse = stream.receive_json(self.operation_timeout()).await?;
        match response {
            DeviceTaskResponse::TerminalOutput {
                session_id: returned,
                retained_from,
                offset: returned_offset,
                next_offset,
                size,
                ended,
            } if returned == session_id
                && retained_from <= returned_offset
                && returned_offset >= offset
                && next_offset == returned_offset + u64::from(size)
                && size as usize <= MAX_TERMINAL_READ_BYTES =>
            {
                let bytes = if size == 0 {
                    Vec::new()
                } else {
                    stream
                        .receive_binary_frame(self.operation_timeout())
                        .await?
                };
                if bytes.len() != size as usize {
                    return Err(BridgeError::Terminal(
                        "terminal output size mismatch".to_owned(),
                    ));
                }
                Ok(TerminalOutput {
                    retained_from,
                    offset: returned_offset,
                    next_offset,
                    bytes,
                    ended,
                })
            }
            DeviceTaskResponse::Error { code, message } => {
                Err(BridgeError::RemoteTask { code, message })
            }
            response => Err(unexpected_task_response(response)),
        }
    }

    pub async fn terminal_resize(
        &self,
        session_id: RequestId,
        sequence: u64,
        cols: u16,
        rows: u16,
    ) -> Result<(), BridgeError> {
        let response = self
            .task_request(DeviceTaskRequest::TerminalResize {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                session_id,
                sequence,
                cols,
                rows,
            })
            .await?;
        terminal_ack(response, session_id, sequence)
    }

    pub async fn terminal_close(
        &self,
        session_id: RequestId,
        sequence: u64,
    ) -> Result<(), BridgeError> {
        let response = self
            .task_request(DeviceTaskRequest::TerminalClose {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                session_id,
                sequence,
            })
            .await?;
        match response {
            DeviceTaskResponse::TerminalClosed {
                session_id: returned,
            } if returned == session_id => Ok(()),
            response => Err(unexpected_task_response(response)),
        }
    }
}

fn terminal_ack(
    response: DeviceTaskResponse,
    session_id: RequestId,
    sequence: u64,
) -> Result<(), BridgeError> {
    match response {
        DeviceTaskResponse::TerminalAcknowledged {
            session_id: returned,
            sequence: returned_sequence,
        } if returned == session_id && returned_sequence == sequence => Ok(()),
        response => Err(unexpected_task_response(response)),
    }
}
