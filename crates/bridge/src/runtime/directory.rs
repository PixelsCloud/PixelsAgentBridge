use pab_protocol::{DeviceRef, DirectoryPage, RequestId};

use super::{BridgeRuntime, RuntimeError};

impl BridgeRuntime {
    pub async fn list_directory_as(
        &self,
        device_ref: DeviceRef,
        path: &str,
        after: Option<&str>,
        limit: u16,
        execution: pab_protocol::ExecutionSelection,
    ) -> Result<DirectoryPage, RuntimeError> {
        if execution.is_service() {
            return self.list_directory(device_ref, path, after, limit).await;
        }
        let request = pab_protocol::FileSystemRequest {
            execution,
            request_id: RequestId::new(),
            path: path.to_owned(),
            operation: pab_protocol::FileSystemAction::ListDirectory {
                after: after.map(str::to_owned),
                limit,
            },
            payload_size: 0,
            payload_sha256: None,
        };
        let result = self.filesystem(device_ref, &request, &[]).await?;
        if result.reply.state == "completed" {
            if let Some(page) = result.reply.directory {
                return Ok(page);
            }
        }
        Err(
            crate::BridgeError::UnexpectedTaskResponse(result.reply.error.map_or_else(
                || "directory result was not confirmed".into(),
                |error| format!("{}: {}", error.code, error.message),
            ))
            .into(),
        )
    }

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
        self.inner.wait_account_ready().await?;
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
