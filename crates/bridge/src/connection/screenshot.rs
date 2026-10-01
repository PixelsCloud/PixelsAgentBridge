use super::{AuthenticatedDeviceConnection, BridgeError, unexpected_task_response};
use pab_protocol::{
    DEVICE_TASK_SCHEMA_VERSION, DeviceTaskRequest, DeviceTaskResponse, RequestId, ScreenshotFormat,
    ScreenshotInfo, ScreenshotMeta, ScreenshotMode, ScreenshotOptions,
};
use sha2::{Digest, Sha256};

pub struct Screenshot {
    pub meta: ScreenshotMeta,
    pub bytes: Vec<u8>,
}

impl AuthenticatedDeviceConnection {
    /// Legacy primary-monitor PNG for clients that have not negotiated screenshot options.
    pub async fn capture_screenshot(
        &self,
        request_id: RequestId,
    ) -> Result<Screenshot, BridgeError> {
        self.screenshot_request(request_id, None).await
    }
    pub async fn capture_screenshot_with_options(
        &self,
        request_id: RequestId,
        options: &ScreenshotOptions,
    ) -> Result<Screenshot, BridgeError> {
        options
            .validate()
            .map_err(|e| BridgeError::FileTransfer(e.into()))?;
        match self
            .task_request(DeviceTaskRequest::GetEnvironment {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
            })
            .await?
        {
            DeviceTaskResponse::Environment {
                context,
                screenshot_schema_version,
                ..
            } if context.device_ref == self.device_ref => {
                if screenshot_schema_version.is_none_or(|v| v < options.required_version()) {
                    return Err(BridgeError::FileTransfer("target does not support screenshot options; upgrade its Executor and desktop helper".into()));
                }
            }
            response => return Err(unexpected_task_response(response)),
        }
        self.screenshot_request(request_id, Some(options)).await
    }
    async fn screenshot_request(
        &self,
        request_id: RequestId,
        options: Option<&ScreenshotOptions>,
    ) -> Result<Screenshot, BridgeError> {
        tokio::time::timeout(
            self.operation_timeout()
                .max(std::time::Duration::from_secs(30)),
            self.receive_screenshot(request_id, options),
        )
        .await
        .map_err(|_| {
            BridgeError::FileTransfer("screenshot request exceeded its overall deadline".into())
        })?
    }

    async fn receive_screenshot(
        &self,
        request_id: RequestId,
        options: Option<&ScreenshotOptions>,
    ) -> Result<Screenshot, BridgeError> {
        let timeout = self
            .operation_timeout()
            .max(std::time::Duration::from_secs(30));
        let mut stream = self.connection.open_bi(timeout).await?;
        let request = match options {
            Some(options) => DeviceTaskRequest::CaptureScreenshotV2 {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id,
                options: options.clone(),
            },
            None => DeviceTaskRequest::CaptureScreenshot {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id,
            },
        };
        stream.send_json(&request, timeout).await?;
        let response: DeviceTaskResponse = stream.receive_json(timeout).await?;
        let meta = match response {
            DeviceTaskResponse::Screenshot { meta } if valid_meta(&meta, request_id, options) => {
                meta
            }
            DeviceTaskResponse::Error { code, message } => {
                return Err(BridgeError::RemoteTask { code, message });
            }
            response => return Err(unexpected_task_response(response)),
        };
        let size = usize::try_from(meta.size).map_err(|_| {
            BridgeError::FileTransfer("screenshot length is not representable".into())
        })?;
        let mut bytes = Vec::new();
        let mut frames = 0;
        while bytes.len() < size {
            let chunk = stream.receive_binary_frame(timeout).await?;
            if verification_is_legacy(options) {
                frames += 1;
            }
            if chunk.is_empty()
                || (verification_is_legacy(options) && frames > 4096)
                || chunk.len() > size - bytes.len()
            {
                return Err(BridgeError::FileTransfer(
                    "invalid screenshot binary frame".into(),
                ));
            }
            bytes.try_reserve(chunk.len()).map_err(|_| {
                BridgeError::FileTransfer("insufficient memory receiving screenshot".into())
            })?;
            bytes.extend_from_slice(&chunk);
        }
        stream.expect_receive_end(timeout).await?;
        let verification_options = options.cloned().unwrap_or_else(ScreenshotOptions::original);
        tokio::task::spawn_blocking(move || {
            if format!("{:x}", Sha256::digest(&bytes)) != meta.sha256 {
                return Err(BridgeError::FileTransfer(
                    "screenshot SHA-256 mismatch".into(),
                ));
            }
            let info = meta.capture.clone().unwrap_or(ScreenshotInfo {
                window_ref: None,
                desktop_rect: None,
                captured_at_unix_ms: 0,
                mode: ScreenshotMode::Original,
                format: ScreenshotFormat::Png,
                width: meta.width,
                height: meta.height,
                source_width: meta.width,
                source_height: meta.height,
                monitor_id: None,
                origin_x: 0,
                origin_y: 0,
                quality: None,
                resized: false,
            });
            pab_screenshot::verify(&bytes, &info, &verification_options)
                .map_err(BridgeError::FileTransfer)?;
            Ok(Screenshot { meta, bytes })
        })
        .await
        .map_err(|e| BridgeError::FileTransfer(e.to_string()))?
    }
}
fn verification_is_legacy(options: Option<&ScreenshotOptions>) -> bool {
    options.is_none_or(|o| o.mode != ScreenshotMode::Jpeg)
}
fn valid_meta(meta: &ScreenshotMeta, id: RequestId, options: Option<&ScreenshotOptions>) -> bool {
    let original = ScreenshotOptions::original();
    let limits = options.unwrap_or(&original);
    if meta.request_id != id
        || meta.size == 0
        || meta.size > limits.byte_limit() as u64
        || meta.width == 0
        || meta.height == 0
        || (limits.mode != ScreenshotMode::Jpeg
            && u64::from(meta.width) * u64::from(meta.height) > pab_protocol::MAX_SCREENSHOT_PIXELS)
        || meta.sha256.len() != 64
        || !meta.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        || meta.format != limits.format().name()
    {
        return false;
    }
    match (options, &meta.capture) {
        (Some(options), Some(info)) => {
            info.validate(options).is_ok() && (info.width, info.height) == (meta.width, meta.height)
        }
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pab_protocol::{DeploymentId, DeviceId, DeviceRef, EndpointKey, OperatorRef, TenantId};
    use pab_transport::{PabConnection, PabEndpoint, PabEndpointAddress, PabEndpointConfig};
    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    async fn pair() -> (
        PabEndpoint,
        PabEndpoint,
        AuthenticatedDeviceConnection,
        PabConnection,
    ) {
        let config = PabEndpointConfig::new(vec!["https://127.0.0.1:1".parse().unwrap()]).unwrap();
        let key = || {
            iroh_base::SecretKey::from_bytes(
                &Sha256::digest(RequestId::new().to_string().as_bytes()).into(),
            )
        };
        let first = PabEndpoint::bind(config.clone(), key()).await.unwrap();
        let second = PabEndpoint::bind(config, key()).await.unwrap();
        let mut addresses = second.watch_address();
        tokio::time::timeout(TIMEOUT, async {
            while addresses.borrow().direct_addresses.is_empty() {
                addresses.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        let direct = addresses
            .borrow()
            .direct_addresses
            .iter()
            .map(|address| {
                std::net::SocketAddr::new(
                    if address.is_ipv4() {
                        std::net::Ipv4Addr::LOCALHOST.into()
                    } else {
                        std::net::Ipv6Addr::LOCALHOST.into()
                    },
                    address.port(),
                )
            })
            .collect();
        let remote = second.clone();
        let accept = tokio::spawn(async move { remote.accept().await.unwrap().unwrap() });
        let client = first
            .connect(
                *second.id().as_bytes(),
                &PabEndpointAddress {
                    relay_urls: vec![],
                    direct_addresses: direct,
                },
                TIMEOUT,
            )
            .await
            .unwrap();
        let server = accept.await.unwrap();
        let authenticated = AuthenticatedDeviceConnection {
            connection: client,
            device_ref: DeviceRef {
                deployment_id: DeploymentId::from_u128(1),
                tenant_id: TenantId::from_u128(2),
                device_id: DeviceId::from_u128(3),
            },
            operator: OperatorRef::guest(EndpointKey::new([1; 32])),
            password_version: 1,
            authenticated_at_unix_ms: 1,
            operation_timeout: TIMEOUT,
        };
        (first, second, authenticated, server)
    }
    #[tokio::test]
    async fn receive_stream_rejects_wrong_hash_truncation_extra_bytes_and_wrong_codec() {
        let (first, second, client, remote) = pair().await;
        for fault in ["none", "hash", "truncated", "extra", "codec"] {
            let id = RequestId::new();
            let options = ScreenshotOptions::default();
            let image = pab_screenshot::encode(
                pab_screenshot::image::DynamicImage::new_rgb8(100, 80),
                &options,
                Some(1),
                (0, 0),
            )
            .unwrap();
            let remote = remote.clone();
            let handler = tokio::spawn(async move {
                let mut stream = remote.accept_bi(TIMEOUT).await.unwrap();
                let request: DeviceTaskRequest = stream.receive_json(TIMEOUT).await.unwrap();
                assert!(
                    matches!(request,DeviceTaskRequest::CaptureScreenshotV2 {request_id,..} if request_id==id)
                );
                let mut bytes = image.bytes;
                if fault == "codec" {
                    bytes.truncate(bytes.len() - 2);
                }
                let meta = ScreenshotMeta {
                    request_id: id,
                    format: "jpeg".into(),
                    width: 100,
                    height: 80,
                    size: bytes.len() as u64,
                    sha256: if fault == "hash" {
                        "0".repeat(64)
                    } else {
                        format!("{:x}", Sha256::digest(&bytes))
                    },
                    capture: Some(image.info),
                };
                stream
                    .send_frame_json(&DeviceTaskResponse::Screenshot { meta }, TIMEOUT)
                    .await
                    .unwrap();
                if fault == "truncated" {
                    bytes.truncate(bytes.len() / 2);
                }
                stream.send_binary_frame(&bytes, TIMEOUT).await.unwrap();
                if fault == "extra" {
                    let _ = stream.send_binary_frame(b"extra", TIMEOUT).await;
                }
                let _ = stream.finish_send(TIMEOUT).await;
            });
            let received = client.screenshot_request(id, Some(&options)).await;
            assert_eq!(received.is_ok(), fault == "none", "fault {fault}");
            handler.await.unwrap();
        }
        first.close().await;
        second.close().await;
    }
    #[test]
    fn reply_identity_format_dimensions_hash_and_budget_must_match() {
        let options = ScreenshotOptions::default();
        let encoded = pab_screenshot::encode(
            pab_screenshot::image::DynamicImage::new_rgb8(10, 20),
            &options,
            Some(1),
            (0, 0),
        )
        .unwrap();
        let id = RequestId::new();
        let meta = ScreenshotMeta {
            request_id: id,
            format: "jpeg".into(),
            width: 10,
            height: 20,
            size: encoded.bytes.len() as u64,
            sha256: "a".repeat(64),
            capture: Some(encoded.info),
        };
        assert!(valid_meta(&meta, id, Some(&options)));
        assert!(!valid_meta(&meta, RequestId::new(), Some(&options)));
        assert!(!valid_meta(&meta, id, None));
        for field in ["format", "size", "hash", "dimensions", "metadata"] {
            let mut wrong = meta.clone();
            match field {
                "format" => wrong.format = "png".into(),
                "size" => wrong.size = 0,
                "hash" => wrong.sha256 = "z".repeat(64),
                "dimensions" => wrong.width += 1,
                _ => wrong.capture = None,
            }
            assert!(!valid_meta(&wrong, id, Some(&options)), "{field}");
        }
    }
}
