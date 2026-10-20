use super::*;
impl BridgeRuntime {
    /// Reuse the current account's authenticated device connection. A media
    /// error does not invalidate unrelated task/terminal/file streams.
    pub async fn open_remote_desktop(
        &self,
        device: DeviceRef,
        screen: Option<String>,
        fps: u16,
    ) -> Result<crate::RemoteDesktopSession, RuntimeError> {
        self.inner.wait_account_ready().await?;
        let device = self.inner.device(device).await;
        let connection = device.connection().await?;
        match connection.open_remote_desktop(screen, fps).await {
            Ok(session) => Ok(session),
            Err(error) => {
                // A media/helper failure must not tear down working terminal or
                // file streams. Evict only a transport that is already closed;
                // the viewer's bounded retry can then authenticate a new one.
                if connection.connection().selected_path().is_none() {
                    device.recover(&connection, &error).await?;
                }
                Err(error.into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pab_protocol::{DeviceTaskRequest, DeviceTaskResponse};

    #[tokio::test]
    async fn desktop_open_evicts_closed_transport_but_preserves_live_legacy_connection() {
        for closed in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let runtime = super::super::test_support::runtime(dir.path()).await;
            let (_a, _b, client, server) = crate::connection::test_connection_pair().await;
            let device_ref = client.device_ref();
            let transport = client.connection().clone();
            let device = runtime.inner.device(device_ref).await;
            device.set_test_connection(client).await;
            let peer = tokio::spawn(async move {
                if closed {
                    server.close(b"service restarted");
                    return server;
                }
                let timeout = Duration::from_secs(5);
                let mut stream = server.accept_bi(timeout).await.unwrap();
                assert!(matches!(
                    stream
                        .receive_json::<DeviceTaskRequest>(timeout)
                        .await
                        .unwrap(),
                    DeviceTaskRequest::GetEnvironment { .. }
                ));
                // A legacy capability reply is a media failure, not a reason
                // to close the device's terminal/file connection.
                stream
                    .send_json(
                        &DeviceTaskResponse::Error {
                            code: pab_protocol::DeviceTaskErrorCode::Unsupported,
                            message: "fixture legacy device".into(),
                        },
                        timeout,
                    )
                    .await
                    .unwrap();
                server
            });
            if closed {
                tokio::time::timeout(Duration::from_secs(5), transport.closed())
                    .await
                    .unwrap();
            }
            let result = runtime.open_remote_desktop(device_ref, None, 15).await;
            assert!(result.is_err());
            assert_eq!(device.cached_connection().await.is_none(), closed);
            assert_eq!(transport.selected_path().is_none(), closed);
            peer.await.unwrap();
        }
    }
}
