use super::{AuthenticatedDeviceConnection, BridgeError, unexpected_task_response};
use pab_protocol::*;
use sha2::{Digest, Sha256};

#[cfg(test)]
#[path = "mobile_tests.rs"]
mod tests;

impl AuthenticatedDeviceConnection {
    pub async fn mobile(
        &self,
        request: MobileRequest,
    ) -> Result<(MobileReply, Option<Vec<u8>>), BridgeError> {
        let id = request.request_id();
        if let MobileRequest::Execute { command, .. } = &request {
            command
                .validate()
                .map_err(|e| BridgeError::UnexpectedTaskResponse(e.into()))?;
        }
        let (target, supported) = match self
            .task_request(DeviceTaskRequest::GetEnvironment {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
            })
            .await?
        {
            DeviceTaskResponse::Environment {
                context,
                mobile_schema_version,
                ..
            } if context.device_ref == self.device_ref
                && context.source == TargetContextSource::ExecutorVerified
                && context.freshness == ContextFreshness::Current =>
            {
                (*context, mobile_schema_version.unwrap_or(1))
            }
            response => return Err(unexpected_task_response(response)),
        };
        if target.execution.os_family != OsFamily::Android {
            return Err(BridgeError::UnexpectedTaskResponse(
                "mobile control requires Android".into(),
            ));
        }
        let version = supported.min(MOBILE_SCHEMA_VERSION);
        if version < request.required_version() {
            return Err(BridgeError::UnexpectedTaskResponse(
                "mobile capability requires a newer Android client".into(),
            ));
        }
        let mut stream = self.connection.open_bi(self.operation_timeout).await?;
        stream
            .send_json(
                &DeviceTaskRequest::Mobile {
                    schema_version: DEVICE_TASK_SCHEMA_VERSION,
                    mobile_schema_version: version,
                    request,
                },
                self.operation_timeout,
            )
            .await?;
        let response: DeviceTaskResponse = stream.receive_json(self.operation_timeout).await?;
        let DeviceTaskResponse::Mobile { reply } = response else {
            return Err(unexpected_task_response(response));
        };
        if reply.request_id != id
            || reply.error.as_ref().is_some_and(|v| v.len() > 128)
            || reply.observation.as_ref().is_some_and(|v| !v.validate())
            || reply.capabilities.as_ref().is_some_and(|v| !v.validate())
        {
            return Err(BridgeError::UnexpectedTaskResponse(
                "invalid mobile response".into(),
            ));
        }
        let mut bytes = None;
        if let Some(image) = &reply.image {
            if image.size == 0
                || image.size > 8 * 1024 * 1024
                || image.width == 0
                || image.height == 0
                || u64::from(image.width) * u64::from(image.height) > 16_000_000
            {
                return Err(BridgeError::UnexpectedTaskResponse(
                    "invalid mobile image".into(),
                ));
            }
            let mut received = Vec::with_capacity(image.size as usize);
            while received.len() < image.size as usize {
                let frame = stream.receive_binary_frame(self.operation_timeout).await?;
                if received.len() + frame.len() > image.size as usize {
                    return Err(BridgeError::UnexpectedTaskResponse(
                        "mobile image overflow".into(),
                    ));
                }
                received.extend_from_slice(&frame);
            }
            if !received.starts_with(&[0xff, 0xd8])
                || format!("{:x}", Sha256::digest(&received)) != image.sha256
            {
                return Err(BridgeError::UnexpectedTaskResponse(
                    "mobile image integrity mismatch".into(),
                ));
            }
            let width = image.width;
            let height = image.height;
            received = tokio::task::spawn_blocking(move || {
                let info = ScreenshotInfo {
                    window_ref: None,
                    desktop_rect: None,
                    captured_at_unix_ms: 0,
                    mode: ScreenshotMode::Jpeg,
                    format: ScreenshotFormat::Jpeg,
                    width,
                    height,
                    source_width: width,
                    source_height: height,
                    monitor_id: None,
                    origin_x: 0,
                    origin_y: 0,
                    quality: Some(80),
                    resized: false,
                };
                let options = ScreenshotOptions {
                    quality: Some(80),
                    ..Default::default()
                };
                pab_screenshot::verify(&received, &info, &options)
                    .map_err(BridgeError::FileTransfer)?;
                Ok::<_, BridgeError>(received)
            })
            .await
            .map_err(|_| BridgeError::FileTransfer("image verifier failed".into()))??;
            bytes = Some(received);
        }
        stream.expect_receive_end(self.operation_timeout).await?;
        Ok((*reply, bytes))
    }
}
