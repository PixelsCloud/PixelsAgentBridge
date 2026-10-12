use super::{BridgeRuntime, RuntimeError};
use pab_protocol::{DeviceRef, DocumentReply, DocumentRequest, RequestId};

impl BridgeRuntime {
    pub async fn document(&self, device: DeviceRef, request: DocumentRequest) -> Result<DocumentReply, RuntimeError> {
        self.inner.wait_account_ready().await?;
        Ok(self.inner.device(device).await.connection().await?.document(request).await?)
    }

    pub async fn get_document(&self, device: DeviceRef, id: RequestId) -> Result<DocumentReply, RuntimeError> {
        self.inner.wait_account_ready().await?;
        Ok(self.inner.device(device).await.connection().await?.get_document(id).await?)
    }

    pub async fn cancel_document(&self, device: DeviceRef, id: RequestId) -> Result<DocumentReply, RuntimeError> {
        self.inner.wait_account_ready().await?;
        Ok(self.inner.device(device).await.connection().await?.cancel_document(id).await?)
    }
}
