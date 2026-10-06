use pab_protocol::{
    DEVICE_TASK_SCHEMA_VERSION, DeviceTaskRequest, DeviceTaskResponse, FileSystemReply,
    FileSystemRequest, MAX_TEXT_READ_BYTES, RequestId, valid_file_hash,
};
use sha2::{Digest, Sha256};

use super::{AuthenticatedDeviceConnection, BridgeError, unexpected_task_response};

pub struct FileSystemResult {
    pub reply: FileSystemReply,
    pub data: Vec<u8>,
}

impl AuthenticatedDeviceConnection {
    pub async fn filesystem(
        &self,
        request: &FileSystemRequest,
        payload: &[u8],
    ) -> Result<FileSystemResult, BridgeError> {
        request
            .validate()
            .map_err(|error| BridgeError::UnexpectedTaskResponse(error.to_owned()))?;
        if payload.len() != request.payload_size as usize
            || request.operation.has_payload()
                && request.payload_sha256.as_deref()
                    != Some(format!("{:x}", Sha256::digest(payload)).as_str())
        {
            return Err(BridgeError::UnexpectedTaskResponse(
                "invalid local text payload".to_owned(),
            ));
        }
        let timeout = self.operation_timeout();
        let capabilities = self
            .task_request(DeviceTaskRequest::GetEnvironment {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
            })
            .await?;
        match capabilities {
            DeviceTaskResponse::Environment {
                context,
                filesystem_schema_version,
                ..
            } if context.device_ref == self.device_ref => {
                if !supports(filesystem_schema_version, request.schema_version()) {
                    return Err(BridgeError::UnsupportedFileSystem);
                }
            }
            response => return Err(unexpected_task_response(response)),
        }
        let mut stream = self.connection.open_bi(timeout).await?;
        stream
            .send_frame_json(
                &DeviceTaskRequest::FileSystem {
                    schema_version: DEVICE_TASK_SCHEMA_VERSION,
                    request: request.clone(),
                },
                timeout,
            )
            .await?;
        for chunk in payload.chunks(64 * 1024) {
            stream.send_binary_frame(chunk, timeout).await?;
        }
        stream.finish_send(timeout).await?;
        let response: DeviceTaskResponse = stream.receive_json(timeout).await?;
        let DeviceTaskResponse::FileSystem { reply } = response else {
            return Err(unexpected_task_response(response));
        };
        let reply = *reply;
        if !valid_reply(&reply, request.request_id)
            || reply.path != request.path
            || reply.kind != request.operation.kind()
            || reply.destination.as_deref() != request.operation.destination()
            || (reply.data_size > 0 && reply.kind != "file_read")
            || (!request.execution.is_service()
                && reply.state != "failed"
                && reply
                    .execution_context
                    .as_ref()
                    .and_then(|c| c.identity.as_ref())
                    .is_none_or(|id| {
                        id.mode != pab_protocol::ExecutionMode::User || id.validate().is_err()
                    }))
        {
            return Err(BridgeError::UnexpectedTaskResponse(
                "filesystem reply does not match request".to_owned(),
            ));
        }
        let mut data = Vec::with_capacity(reply.data_size as usize);
        while data.len() < reply.data_size as usize {
            let bytes = stream.receive_binary_frame(timeout).await?;
            if bytes.is_empty() || data.len() + bytes.len() > reply.data_size as usize {
                return Err(BridgeError::UnexpectedTaskResponse(
                    "invalid text response frame".to_owned(),
                ));
            }
            data.extend_from_slice(&bytes);
        }
        stream.expect_receive_end(timeout).await?;
        if (reply
            .data_sha256
            .as_deref()
            .is_some_and(|expected| expected != format!("{:x}", Sha256::digest(&data))))
            || std::str::from_utf8(&data).is_err()
        {
            return Err(BridgeError::UnexpectedTaskResponse(
                "text response integrity check failed".to_owned(),
            ));
        }
        Ok(FileSystemResult { reply, data })
    }

    pub async fn cancel_filesystem(&self, id: RequestId) -> Result<FileSystemReply, BridgeError> {
        let response = self
            .task_request(DeviceTaskRequest::CancelFileSystem {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id: id,
            })
            .await?;
        match response {
            DeviceTaskResponse::FileSystem { reply }
                if valid_reply(&reply, id)
                    && pab_protocol::cancellable_filesystem_kind(&reply.kind)
                    && reply.data_size == 0 =>
            {
                Ok(*reply)
            }
            response => Err(unexpected_task_response(response)),
        }
    }

    pub async fn get_filesystem(&self, id: RequestId) -> Result<FileSystemReply, BridgeError> {
        let response = self
            .task_request(DeviceTaskRequest::GetFileSystem {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id: id,
            })
            .await?;
        match response {
            DeviceTaskResponse::FileSystem { reply }
                if valid_reply(&reply, id) && reply.data_size == 0 =>
            {
                Ok(*reply)
            }
            response => Err(unexpected_task_response(response)),
        }
    }
}

fn valid_reply(reply: &FileSystemReply, id: RequestId) -> bool {
    serde_json::to_vec(reply).is_ok_and(|bytes| bytes.len() <= 32 * 1024)
        && reply.request_id == id
        && reply.path.len() <= 4096
        && reply.destination.as_ref().is_none_or(|p| p.len() <= 4096)
        && matches!(
            reply.state.as_str(),
            "running"
                | "completed"
                | "failed"
                | "unconfirmed"
                | "interrupted"
                | "cancel_requested"
                | "cancelled"
        )
        && reply.data_size as usize <= MAX_TEXT_READ_BYTES
        && reply
            .metadata
            .as_ref()
            .is_none_or(|meta| meta.sha256.as_deref().is_none_or(valid_file_hash))
        && reply
            .error
            .as_ref()
            .is_none_or(|error| error.message.len() <= 8192)
        && reply.data_sha256.as_deref().is_none_or(valid_file_hash)
        && (reply.data_size == 0 || reply.data_sha256.is_some())
}

fn supports(version: Option<u16>, required: u16) -> bool {
    version.is_some_and(|version| version >= required)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pab_protocol::FileSystemAction;
    #[test]
    fn v2_peer_rejects_bulk_tools_before_any_mutation_is_sent() {
        use pab_protocol::FileOperationLimits;
        let actions = [
            FileSystemAction::Copy {
                destination: "/out".into(),
                recursive: false,
                overwrite: false,
                limits: FileOperationLimits::default(),
            },
            FileSystemAction::Move {
                destination: "/out".into(),
                recursive: false,
                overwrite: false,
                limits: FileOperationLimits::default(),
            },
            FileSystemAction::Delete {
                recursive: false,
                limits: FileOperationLimits::default(),
            },
            FileSystemAction::ArchiveCreate {
                sources: vec!["/in".into()],
                overwrite: false,
                limits: FileOperationLimits::default(),
            },
            FileSystemAction::ArchiveExtract {
                destination: "/out".into(),
                overwrite: false,
                max_ratio: 200,
                limits: FileOperationLimits::default(),
            },
        ];
        for action in actions {
            assert!(!supports(Some(2), action.schema_version()));
            assert!(supports(Some(3), action.schema_version()));
        }
    }
    #[test]
    fn v1_peer_supports_existing_tools_but_b2_is_rejected_before_sending() {
        assert!(supports(
            Some(1),
            FileSystemAction::Stat {
                follow_symlinks: false
            }
            .schema_version()
        ));
        for action in [
            FileSystemAction::Hash,
            FileSystemAction::Mkdir {
                parents: false,
                exist_ok: false,
            },
        ] {
            assert!(!supports(None, action.schema_version()));
            assert!(!supports(Some(1), action.schema_version()));
            assert!(supports(Some(2), action.schema_version()));
        }
        let request = FileSystemRequest {
            execution: Default::default(),
            request_id: RequestId::new(),
            path: "/tmp/hash.bin".to_owned(),
            operation: FileSystemAction::Hash,
            payload_size: 0,
            payload_sha256: None,
        };
        let mut reply = FileSystemReply::pending(&request);
        reply.state = "cancel_requested".to_owned();
        assert!(valid_reply(&reply, request.request_id));
        reply.state = "cancelled".to_owned();
        assert!(valid_reply(&reply, request.request_id));
        assert!(!valid_reply(&reply, RequestId::new()));
        reply.created_paths = Some(vec!["x".repeat(40 * 1024)]);
        assert!(!valid_reply(&reply, request.request_id));
    }
}
