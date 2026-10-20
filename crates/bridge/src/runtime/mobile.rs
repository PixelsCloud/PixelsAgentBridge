use super::{BridgeRuntime, RuntimeError};
use pab_protocol::{DeviceRef, MobileReply, MobileRequest};

impl BridgeRuntime {
    /// Never silently retry a mutation. Callers retain its request ID and query the device.
    pub async fn mobile(
        &self,
        device: DeviceRef,
        request: MobileRequest,
    ) -> Result<(MobileReply, Option<Vec<u8>>), RuntimeError> {
        self.inner.wait_account_ready().await?;
        let target = self.inner.device(device).await;
        let connection = target.connection().await?;
        match connection.mobile(request).await {
            Ok(reply) => Ok(reply),
            Err(error) if error.is_recoverable_connection() => {
                // Invalidate the failed transport without resending accepted input.
                // The next get/execute reconnects; the caller keeps the original ID.
                target.recover(&connection, &error).await?;
                Err(error.into())
            }
            Err(error) => Err(error.into()),
        }
    }
}
