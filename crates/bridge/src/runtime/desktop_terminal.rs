use super::*;
impl BridgeRuntime {
    pub async fn open_desktop_terminal(
        &self,
        device: DeviceRef,
        execution: pab_protocol::ExecutionSelection,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<crate::DesktopTerminalSession>, RuntimeError> {
        self.inner.wait_account_ready().await?;
        self.current_environment(device).await?;
        let target = self.inner.device(device).await;
        let connection = target.connection().await?;
        Ok(connection
            .open_desktop_terminal(execution, cols, rows)
            .await?)
    }
}
