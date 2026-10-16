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
        let connection = self.inner.device(device).await.connection().await?;
        Ok(connection.open_remote_desktop(screen, fps).await?)
    }
}
