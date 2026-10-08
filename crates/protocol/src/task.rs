use serde::{Deserialize, Serialize};

use crate::{
    ContextFreshness, DeviceRef, ExecutionContext, OperatorRef, RequestId, TargetContext,
    TargetContextSource, TaskId,
};

pub const TASK_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskRef {
    pub device_ref: DeviceRef,
    pub task_id: TaskId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRef {
    pub name: String,
    pub version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Accepted,
    Running,
    CancelRequested,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

impl TaskState {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    ToExecutor,
    FromExecutor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferPhase {
    Preparing,
    Transferring,
    Verifying,
    Committing,
    AwaitingReceiverConfirmation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferProgress {
    pub direction: TransferDirection,
    pub phase: TransferPhase,
    pub confirmed_bytes: u64,
    pub total_bytes: Option<u64>,
    pub sampled_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskProgress {
    Transfer(TransferProgress),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskCompletion {
    pub summary: String,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskError {
    pub code: String,
    pub message: String,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OutputRange {
    pub retained_from: u64,
    pub available_to: u64,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OutputAvailability {
    pub stdout: OutputRange,
    pub stderr: OutputRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputChunk {
    pub schema_version: u16,
    pub task_ref: TaskRef,
    pub stream: OutputStream,
    pub offset: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskEventKind {
    Accepted,
    Running,
    StageChanged { stage: Option<String> },
    Progress { progress: TaskProgress },
    CancelRequested,
    Succeeded { completion: TaskCompletion },
    Failed { error: TaskError },
    Cancelled { reason: String },
    Interrupted { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEvent {
    pub schema_version: u16,
    pub task_ref: TaskRef,
    pub seq: u64,
    pub occurred_at_unix_ms: i64,
    pub kind: TaskEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub initiating_user: Option<crate::UserAttribution>,
    pub schema_version: u16,
    pub task_ref: TaskRef,
    pub request_id: RequestId,
    pub initiated_by: OperatorRef,
    pub capability: CapabilityRef,
    pub display_summary: String,
    pub state: TaskState,
    pub stage: Option<String>,
    pub latest_event_seq: u64,
    pub created_at_unix_ms: i64,
    pub started_at_unix_ms: Option<i64>,
    pub finished_at_unix_ms: Option<i64>,
    pub progress: Option<TaskProgress>,
    pub completion: Option<TaskCompletion>,
    pub error: Option<TaskError>,
    pub output: OutputAvailability,
    pub execution_context: ExecutionContext,
}

impl TaskSnapshot {
    pub fn target_context(&self) -> TargetContext {
        TargetContext {
            device_ref: self.task_ref.device_ref,
            execution: self.execution_context.clone(),
            source: TargetContextSource::HistoricalTask,
            observed_at_unix_ms: self.created_at_unix_ms,
            freshness: ContextFreshness::Historical,
        }
    }
}
