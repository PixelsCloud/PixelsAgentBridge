//! File/transfer routing stays separate from Android UI automation and authentication.
use super::*;
use pab_transport::{PabBiStream, PabConnectionError};

impl FileService {
    pub async fn serve_request(
        self: &Arc<Self>,
        owner: &str,
        peer: &AuthorizedDevicePeer,
        execution: &ExecutionContext,
        request: &DeviceTaskRequest,
        stream: &mut PabBiStream,
    ) -> Result<bool, PabConnectionError> {
        let timeout = Duration::from_secs(8);
        let response = match request {
            DeviceTaskRequest::FileSystem { request, .. } => {
                if request.validate().is_err() {
                    stream
                        .send_json(
                            &DeviceTaskResponse::Error {
                                code: DeviceTaskErrorCode::InvalidRequest,
                                message: "Invalid file request".into(),
                            },
                            timeout,
                        )
                        .await?;
                    return Ok(true);
                }
                let mut payload = Vec::with_capacity(request.payload_size as usize);
                while payload.len() < request.payload_size as usize {
                    let chunk = stream.receive_binary_frame(timeout).await?;
                    if chunk.is_empty()
                        || chunk.len() > 65536
                        || payload.len() + chunk.len() > request.payload_size as usize
                    {
                        return Err(PabConnectionError::Stream(
                            "Invalid file payload frame".into(),
                        ));
                    }
                    payload.extend(chunk);
                }
                stream.expect_receive_end(timeout).await?;
                let (reply, data) = self.request(owner, request.clone(), payload).await;
                stream
                    .send_frame_json(
                        &DeviceTaskResponse::FileSystem {
                            reply: Box::new(reply),
                        },
                        timeout,
                    )
                    .await?;
                for chunk in data.chunks(65536) {
                    stream.send_binary_frame(chunk, timeout).await?;
                }
                stream.finish_send(timeout).await?;
                return Ok(true);
            }
            DeviceTaskRequest::GetFileSystem {
                request_id,
                include_data,
                ..
            } => {
                let (mut reply, data) = if *include_data {
                    self.result(owner, *request_id).await
                } else {
                    (self.lookup(owner, *request_id).await, Vec::new())
                };
                if !include_data {
                    reply.data_size = 0;
                    reply.data_sha256 = None;
                    if let Some(search) = reply.search.as_mut() {
                        for item in &mut search.matches {
                            item.preview = None;
                            item.context.clear();
                        }
                    }
                }
                stream
                    .send_frame_json(
                        &DeviceTaskResponse::FileSystem {
                            reply: Box::new(reply),
                        },
                        timeout,
                    )
                    .await?;
                for chunk in data.chunks(65536) {
                    stream.send_binary_frame(chunk, timeout).await?;
                }
                stream.finish_send(timeout).await?;
                return Ok(true);
            }
            DeviceTaskRequest::CancelFileSystem { request_id, .. } => {
                DeviceTaskResponse::FileSystem {
                    reply: Box::new(self.cancel(owner, *request_id).await),
                }
            }
            DeviceTaskRequest::TransferFile { request, .. } => {
                self.transfer(
                    owner,
                    peer.operator,
                    peer.user_context.user.clone(),
                    execution.clone(),
                    request.clone(),
                    stream,
                )
                .await?;
                return Ok(true);
            }
            DeviceTaskRequest::GetTransfer { request_id, .. } => {
                match self.transfer_snapshot(owner, *request_id).await {
                    Ok(Some(snapshot)) => DeviceTaskResponse::Transfer { snapshot },
                    Err(message) => DeviceTaskResponse::Error {
                        code: DeviceTaskErrorCode::Internal,
                        message: message.into(),
                    },
                    Ok(None) => DeviceTaskResponse::Error {
                        code: DeviceTaskErrorCode::NotFound,
                        message: "Transfer was not found for this caller".into(),
                    },
                }
            }
            DeviceTaskRequest::ListDirectory {
                request_id,
                path,
                after,
                limit,
                ..
            } => {
                match self
                    .directory(*request_id, path, after.clone(), *limit)
                    .await
                {
                    Ok(page) => DeviceTaskResponse::Directory { page },
                    Err(error) => DeviceTaskResponse::Error {
                        code: match error.0.code.as_str() {
                            "access_denied" => DeviceTaskErrorCode::AccessDenied,
                            "not_found" | "mount_not_found" => DeviceTaskErrorCode::NotFound,
                            _ => DeviceTaskErrorCode::InvalidRequest,
                        },
                        message: error.0.code,
                    },
                }
            }
            _ => return Ok(false),
        };
        stream.send_json(&response, timeout).await?;
        Ok(true)
    }
}
