use pab_protocol::{
    DEVICE_TASK_SCHEMA_VERSION, DeviceTaskRequest, DeviceTaskResponse, MAX_SCREENSHOT_BYTES,
    MAX_SCREENSHOT_PIXELS, RequestId, ScreenshotMeta,
};
use sha2::{Digest, Sha256};

use super::{AuthenticatedDeviceConnection, BridgeError, unexpected_task_response};

pub struct Screenshot {
    pub meta: ScreenshotMeta,
    pub bytes: Vec<u8>,
}

impl AuthenticatedDeviceConnection {
    pub async fn capture_screenshot(
        &self,
        request_id: RequestId,
    ) -> Result<Screenshot, BridgeError> {
        let mut stream = self.connection.open_bi(self.operation_timeout()).await?;
        stream
            .send_json(
                &DeviceTaskRequest::CaptureScreenshot {
                    schema_version: DEVICE_TASK_SCHEMA_VERSION,
                    request_id,
                },
                self.operation_timeout(),
            )
            .await?;
        let response: DeviceTaskResponse = stream.receive_json(self.operation_timeout()).await?;
        let meta = match response {
            DeviceTaskResponse::Screenshot { meta }
                if meta.request_id == request_id
                    && meta.format == "png"
                    && meta.width > 0
                    && meta.height > 0
                    && u64::from(meta.width) * u64::from(meta.height) <= MAX_SCREENSHOT_PIXELS
                    && meta.size > 0
                    && meta.size <= MAX_SCREENSHOT_BYTES as u64
                    && meta.sha256.len() == 64 =>
            {
                meta
            }
            DeviceTaskResponse::Error { code, message } => {
                return Err(BridgeError::RemoteTask { code, message });
            }
            response => return Err(unexpected_task_response(response)),
        };
        let mut bytes = Vec::with_capacity(meta.size as usize);
        while bytes.len() < meta.size as usize {
            let chunk = stream
                .receive_binary_frame(self.operation_timeout())
                .await?;
            if bytes.len() + chunk.len() > meta.size as usize {
                return Err(BridgeError::FileTransfer(
                    "screenshot exceeds declared size".to_owned(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.len() < 24
            || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
            || u32::from_be_bytes(bytes[16..20].try_into().unwrap()) != meta.width
            || u32::from_be_bytes(bytes[20..24].try_into().unwrap()) != meta.height
            || format!("{:x}", Sha256::digest(&bytes)) != meta.sha256
        {
            return Err(BridgeError::FileTransfer(
                "screenshot verification failed".to_owned(),
            ));
        }
        Ok(Screenshot { meta, bytes })
    }
}
