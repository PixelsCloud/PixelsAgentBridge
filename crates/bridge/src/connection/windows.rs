use pab_protocol::{
    DEVICE_TASK_SCHEMA_VERSION, DesktopInputEvent, DeviceTaskRequest, DeviceTaskResponse,
    MAX_WINDOW_ENTRIES, MAX_WINDOW_TITLE_BYTES, RequestId, WindowList,
};

use super::{AuthenticatedDeviceConnection, BridgeError, unexpected_task_response};

impl AuthenticatedDeviceConnection {
    pub async fn desktop_input(
        &self,
        request_id: RequestId,
        event: DesktopInputEvent,
    ) -> Result<(), BridgeError> {
        let response = self
            .task_request(DeviceTaskRequest::DesktopInput {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id,
                event,
            })
            .await?;
        match response {
            DeviceTaskResponse::DesktopInputApplied {
                request_id: received,
            } if received == request_id => Ok(()),
            DeviceTaskResponse::Error { code, message } => {
                Err(BridgeError::RemoteTask { code, message })
            }
            response => Err(unexpected_task_response(response)),
        }
    }

    pub async fn list_windows(&self, request_id: RequestId) -> Result<WindowList, BridgeError> {
        let response = self
            .task_request(DeviceTaskRequest::ListWindows {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id,
            })
            .await?;
        match response {
            DeviceTaskResponse::Windows { list }
                if list.request_id == request_id
                    && list.entries.len() <= MAX_WINDOW_ENTRIES
                    && list.entries.iter().all(|entry| {
                        !entry.title.is_empty() && entry.title.len() <= MAX_WINDOW_TITLE_BYTES
                    }) =>
            {
                Ok(list)
            }
            response => Err(unexpected_task_response(response)),
        }
    }
}
