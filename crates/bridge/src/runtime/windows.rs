use pab_protocol::{DesktopInputEvent, DeviceRef, RequestId, WindowList};

use super::{BridgeRuntime, RuntimeError};

impl BridgeRuntime {
    pub async fn desktop_input(
        &self,
        device_ref: DeviceRef,
        event: DesktopInputEvent,
    ) -> Result<(), RuntimeError> {
        let id = RequestId::new();
        let kind = match event {
            DesktopInputEvent::MouseMove { .. } => "mouse_move",
            DesktopInputEvent::MouseButton { .. } => "mouse_button",
            DesktopInputEvent::MouseWheel { .. } => "mouse_wheel",
            DesktopInputEvent::Key { .. } => "key",
            DesktopInputEvent::SecureAttention => "secure_attention",
        };
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
            .start_desktop_input_operation(
                id,
                device_ref,
                device_code,
                &self.inner.initiated_by,
                kind,
                &self.inner.session_id,
            )
            .await?;
        let result = async {
            let connection = self.inner.device(device_ref).await.connection().await?;
            connection
                .desktop_input(id, event)
                .await
                .map_err(RuntimeError::from)
        }
        .await;
        let (state, message) = match &result {
            Ok(()) => ("completed", None),
            Err(error) => ("failed", Some(error.to_string())),
        };
        self.inner
            .store
            .finish_operation(id, state, message.as_deref())
            .await?;
        result
    }

    pub async fn list_windows(&self, device_ref: DeviceRef) -> Result<WindowList, RuntimeError> {
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
            .start_window_operation(
                id,
                device_ref,
                device_code,
                &self.inner.initiated_by,
                &self.inner.session_id,
            )
            .await?;
        let result = async {
            let connection = self.inner.device(device_ref).await.connection().await?;
            connection
                .list_windows(id)
                .await
                .map_err(RuntimeError::from)
        }
        .await;
        if let Ok(list) = &result {
            self.inner
                .store
                .operation_progress(id, list.entries.len() as u64, list.entries.len() as u64)
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
