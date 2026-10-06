use crate::{ExecutionSelection, RequestId, valid_file_hash};
use serde::{Deserialize, Serialize};

pub const FILE_TRANSFER_SCHEMA_VERSION: u16 = 2;
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileTransferOptions {
    #[serde(default)]
    pub execution: ExecutionSelection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_from: Option<RequestId>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileTransferRequest {
    pub request_id: RequestId,
    #[serde(default)]
    pub execution: ExecutionSelection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_from: Option<RequestId>,
    pub operation: FileTransferOperation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "direction", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileTransferOperation {
    Upload {
        path: String,
        size: u64,
        sha256: String,
        overwrite: bool,
    },
    Download {
        path: String,
        offset: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_sha256: Option<String>,
    },
}
impl FileTransferOperation {
    pub fn path(&self) -> &str {
        match self {
            Self::Upload { path, .. } | Self::Download { path, .. } => path,
        }
    }
    pub fn direction(&self) -> &'static str {
        match self {
            Self::Upload { .. } => "receive",
            Self::Download { .. } => "send",
        }
    }
}
impl FileTransferRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !matches!(
            self.execution,
            ExecutionSelection::Service {} | ExecutionSelection::User { .. }
        ) {
            return Err("transfer supports service or user execution only");
        }
        if self.resume_from == Some(self.request_id) {
            return Err("resume_from must reference another request");
        }
        let path = self.operation.path();
        if path.is_empty() || path.len() > 4096 || path.contains('\0') {
            return Err("invalid transfer path");
        }
        match &self.operation {
            FileTransferOperation::Upload { size, sha256, .. }
                if *size > i64::MAX as u64 || !valid_file_hash(sha256) =>
            {
                Err("invalid upload size or sha256")
            }
            FileTransferOperation::Download {
                offset,
                expected_sha256,
                ..
            } if *offset > i64::MAX as u64
                || expected_sha256
                    .as_deref()
                    .is_some_and(|v| !valid_file_hash(v)) =>
            {
                Err("invalid download offset or sha256")
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transfer_execution_has_strict_identity_and_distinct_wire_variant() {
        let value = serde_json::json!({"request_id":RequestId::new(),"execution":{"mode":"user","context_ref":crate::ExecutionContextRef::new()},"operation":{"direction":"upload","path":"/tmp/new.bin","size":0,"sha256":"0".repeat(64),"overwrite":false}});
        let request: FileTransferRequest = serde_json::from_value(value.clone()).unwrap();
        request.validate().unwrap();
        let wire = serde_json::to_value(crate::DeviceTaskRequest::TransferFile {
            schema_version: crate::DEVICE_TASK_SCHEMA_VERSION,
            request: request.clone(),
        })
        .unwrap();
        assert_eq!(wire["type"], "transfer_file");
        for field in ["username", "password", "identity"] {
            let mut wrong = value.clone();
            wrong["execution"][field] = serde_json::json!("claimed");
            assert!(serde_json::from_value::<FileTransferRequest>(wrong).is_err());
        }
        let mut invalid = request;
        invalid.resume_from = Some(invalid.request_id);
        assert!(invalid.validate().is_err());
        invalid.resume_from = None;
        invalid.execution = ExecutionSelection::DesktopUser {
            context_ref: crate::ExecutionContextRef::new(),
        };
        assert!(invalid.validate().is_err());
    }
}
