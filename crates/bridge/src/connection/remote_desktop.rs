use super::*;

impl AuthenticatedDeviceConnection {
    pub async fn open_remote_desktop(
        &self,
        screen_id: Option<String>,
        fps: u16,
    ) -> Result<RemoteDesktopSession, BridgeError> {
        match self
            .task_request(DeviceTaskRequest::GetEnvironment {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
            })
            .await?
        {
            DeviceTaskResponse::Environment {
                context,
                remote_desktop_schema_version: Some(pab_protocol::REMOTE_DESKTOP_SCHEMA_VERSION),
                ..
            } if context.device_ref == self.device_ref => {}
            _ => {
                return Err(pab_remote_desktop::DesktopError::Rejected {
                    code: pab_protocol::DesktopErrorCode::Unsupported,
                    message: "this device needs a remote-desktop-capable media backend/update"
                        .into(),
                }
                .into());
            }
        }
        let session = RemoteDesktopSession::open(
            self.connection.clone(),
            screen_id,
            fps,
            self.operation_timeout(),
        )
        .await?;
        // Video Ready is sent after the capture starts. During session changes
        // the old helper retires before its replacement registers; the initial
        // environment can therefore report no cursor even though Open succeeds.
        // Query again so optional support follows the actual active capture,
        // including replacement by a legacy helper that embeds its cursor.
        let cursor_version = match self
            .task_request(DeviceTaskRequest::GetEnvironment {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
            })
            .await
        {
            Ok(DeviceTaskResponse::Environment {
                context,
                remote_desktop_schema_version: Some(pab_protocol::REMOTE_DESKTOP_SCHEMA_VERSION),
                remote_desktop_cursor_schema_version,
                ..
            }) if context.device_ref == self.device_ref => remote_desktop_cursor_schema_version,
            _ => {
                // Cursor discovery must not discard working video/input, nor
                // reuse an earlier capability from a different helper.
                tracing::debug!("desktop cursor capability refresh unavailable");
                None
            }
        };
        Ok(session.with_cursor_support(cursor_version))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    include!("remote_desktop_tests.rs");
    #[tokio::test]
    async fn old_device_is_rejected_before_opening_any_desktop_stream() {
        let (_a, _b, client, server) = test_connection_pair().await;
        let device = client.device_ref();
        let serving = tokio::spawn(async move {
            let mut stream = server.accept_bi(Duration::from_secs(5)).await.unwrap();
            assert!(matches!(
                stream
                    .receive_json::<DeviceTaskRequest>(Duration::from_secs(5))
                    .await
                    .unwrap(),
                DeviceTaskRequest::GetEnvironment { .. }
            ));
            // Legacy environment wire intentionally omits the additive capability.
            let context = serde_json::json!({"device_ref":device,"execution":{"os_family":"windows","os_name":"Windows","os_version":"fixture","architecture":"x86_64","execution_scope":"native","path_style":"windows","interpreter":null,"cwd":"C:\\fixture","environment_revision":"fixture"},"source":"executor_verified","observed_at_unix_ms":1,"freshness":"current"});
            stream.send_json(&serde_json::json!({"type":"environment","context":context,"terminal_schema_version":2}), Duration::from_secs(5)).await.unwrap();
            assert!(
                tokio::time::timeout(
                    Duration::from_millis(150),
                    server.accept_bi(Duration::from_secs(5))
                )
                .await
                .is_err()
            );
        });
        assert!(matches!(
            client.open_remote_desktop(None, 15).await,
            Err(BridgeError::RemoteDesktop(
                pab_remote_desktop::DesktopError::Rejected {
                    code: pab_protocol::DesktopErrorCode::Unsupported,
                    ..
                }
            ))
        ));
        serving.await.unwrap();
    }
}
