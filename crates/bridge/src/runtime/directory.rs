use pab_protocol::{DeviceRef, DirectoryPage, RequestId};

use super::{BridgeRuntime, RuntimeError};

impl BridgeRuntime {
    pub async fn list_directory(
        &self,
        device_ref: DeviceRef,
        path: &str,
        after: Option<&str>,
        limit: u16,
    ) -> Result<DirectoryPage, RuntimeError> {
        let id = RequestId::new();
        let device_code = self
            .inner
            .device_codes
            .lock()
            .await
            .get(&device_ref)
            .copied();
        self.inner
            .store
            .start_directory_operation(
                id,
                device_ref,
                device_code,
                &self.inner.initiated_by,
                path,
                &self.inner.session_id,
            )
            .await?;
        let result = async {
            let connection = self.inner.device(device_ref).await.connection().await?;
            connection
                .list_directory(id, path, after, limit)
                .await
                .map_err(RuntimeError::from)
        }
        .await;
        if let Ok(page) = &result {
            self.inner
                .store
                .operation_progress(id, page.entries.len() as u64, page.entries.len() as u64)
                .await?;
        }
        let (state, message) = match &result {
            Ok(_) => ("completed", None),
            Err(error) => ("failed", Some(error.to_string())),
        };
        self.inner
            .store
            .finish_operation(id, state, message.as_deref())
            .await?;
        result
    }
}
