use super::{AuthenticatedDeviceConnection, BridgeError, unexpected_task_response};
use pab_protocol::{DEVICE_TASK_SCHEMA_VERSION, DOCUMENT_SCHEMA_VERSION, DeviceTaskRequest,
    DeviceTaskResponse, DocumentReply, DocumentRequest, RequestId};

impl AuthenticatedDeviceConnection {
    pub async fn document(&self, request: DocumentRequest) -> Result<DocumentReply, BridgeError> {
        request.validate().map_err(|error| BridgeError::UnexpectedTaskResponse(error.into()))?;
        match self.task_request(DeviceTaskRequest::GetEnvironment {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
        }).await? {
            DeviceTaskResponse::Environment { context, document_schema_version, .. }
                if context.device_ref == self.device_ref && document_schema_version.is_some_and(|v| v >= DOCUMENT_SCHEMA_VERSION) => {}
            DeviceTaskResponse::Environment { .. } => return Err(BridgeError::UnexpectedTaskResponse("document capability unavailable on target".into())),
            response => return Err(unexpected_task_response(response)),
        }
        let id = request.request_id;
        self.document_request(DeviceTaskRequest::Document {
            schema_version: DEVICE_TASK_SCHEMA_VERSION, request,
        }, id).await
    }

    pub async fn get_document(&self, id: RequestId) -> Result<DocumentReply, BridgeError> {
        self.document_request(DeviceTaskRequest::GetDocument {
            schema_version: DEVICE_TASK_SCHEMA_VERSION, request_id: id,
        }, id).await
    }

    pub async fn cancel_document(&self, id: RequestId) -> Result<DocumentReply, BridgeError> {
        self.document_request(DeviceTaskRequest::CancelDocument {
            schema_version: DEVICE_TASK_SCHEMA_VERSION, request_id: id,
        }, id).await
    }

    async fn document_request(&self, request: DeviceTaskRequest, id: RequestId) -> Result<DocumentReply, BridgeError> {
        match self.task_request(request).await? {
            DeviceTaskResponse::Document { reply } if reply.request_id == id &&
                matches!(reply.state.as_str(), "running" | "completed" | "failed" | "cancelled" | "cancel_requested") &&
                serde_json::to_vec(&reply).is_ok_and(|bytes| bytes.len() <= 32 * 1024) => Ok(*reply),
            response => Err(unexpected_task_response(response)),
        }
    }
}
