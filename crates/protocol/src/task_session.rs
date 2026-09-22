use serde::{Deserialize, Serialize};

use crate::{
    ExpectedEnvironment, OutputChunk, OutputRange, OutputStream, RequestId, TargetContext,
    TaskEvent, TaskRef, TaskSnapshot,
};

pub const DEVICE_TASK_SCHEMA_VERSION: u16 = 1;
pub const MAX_COMMAND_PROGRAM_BYTES: usize = 4 * 1024;
pub const MAX_COMMAND_ARGUMENTS: usize = 1024;
pub const MAX_COMMAND_ARGUMENT_BYTES: usize = 16 * 1024;
// JSON encodes bytes as numbers, so leave ample room inside the 64 KiB PAB frame.
pub const MAX_OUTPUT_READ_BYTES: u32 = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandTaskSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub expected_environment: ExpectedEnvironment,
    pub display_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeviceTaskRequest {
    GetEnvironment {
        schema_version: u16,
    },
    SubmitCommand {
        schema_version: u16,
        request_id: RequestId,
        command: CommandTaskSpec,
    },
    GetTask {
        schema_version: u16,
        task_ref: TaskRef,
    },
    ReadOutput {
        schema_version: u16,
        task_ref: TaskRef,
        stream: OutputStream,
        offset: u64,
        max_bytes: u32,
    },
    Subscribe {
        schema_version: u16,
        task_ref: TaskRef,
        after_event_seq: u64,
        stdout_offset: u64,
        stderr_offset: u64,
    },
    Cancel {
        schema_version: u16,
        task_ref: TaskRef,
        reason: String,
    },
}

impl DeviceTaskRequest {
    pub const fn schema_version(&self) -> u16 {
        match self {
            Self::GetEnvironment { schema_version }
            | Self::SubmitCommand { schema_version, .. }
            | Self::GetTask { schema_version, .. }
            | Self::ReadOutput { schema_version, .. }
            | Self::Subscribe { schema_version, .. }
            | Self::Cancel { schema_version, .. } => *schema_version,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceTaskErrorCode {
    InvalidRequest,
    EnvironmentChanged,
    NotFound,
    RequestConflict,
    NotCancellable,
    StorageUnavailable,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeviceTaskResponse {
    Environment {
        context: Box<TargetContext>,
    },
    Submitted {
        snapshot: Box<TaskSnapshot>,
    },
    Snapshot {
        snapshot: Box<TaskSnapshot>,
    },
    Events {
        events: Vec<TaskEvent>,
    },
    Output {
        chunk: OutputChunk,
        range: OutputRange,
    },
    Event {
        event: TaskEvent,
    },
    OutputChanged {
        task_ref: TaskRef,
        stream: OutputStream,
        range: OutputRange,
    },
    CaughtUp {
        snapshot: Box<TaskSnapshot>,
    },
    CancelAccepted {
        snapshot: Box<TaskSnapshot>,
    },
    Error {
        code: DeviceTaskErrorCode,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use crate::{DeploymentId, DeviceId, DeviceRef, TaskId, TenantId};

    use super::*;

    #[test]
    fn task_requests_are_explicitly_versioned() {
        let request = DeviceTaskRequest::GetTask {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
            task_ref: TaskRef {
                device_ref: DeviceRef {
                    deployment_id: DeploymentId::from_u128(1),
                    tenant_id: TenantId::from_u128(2),
                    device_id: DeviceId::from_u128(3),
                },
                task_id: TaskId::from_u128(4),
            },
        };
        let encoded = serde_json::to_vec(&request).unwrap();
        let decoded: DeviceTaskRequest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, request);
        assert_eq!(decoded.schema_version(), DEVICE_TASK_SCHEMA_VERSION);
    }
}
