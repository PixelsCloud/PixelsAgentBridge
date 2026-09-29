use pab_protocol::{
    DeviceRef, OutputChunk, OutputRange, OutputStream, RequestId, TargetContext, TaskEvent,
    TaskRef, TaskSnapshot,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub sequence: u64,
    pub occurred_at_unix_ms: i64,
    pub kind: RuntimeEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceConnectionPhase {
    WaitingForBridge,
    Connecting,
    Connected,
    Disconnected,
    Retrying,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEventKind {
    BridgeConnecting,
    BridgeConnected,
    BridgeRetrying {
        retry_in_ms: u64,
        message: String,
    },
    BridgeStopped {
        message: String,
    },
    DeviceConnection {
        device_ref: DeviceRef,
        phase: DeviceConnectionPhase,
        retry_in_ms: Option<u64>,
        message: Option<String>,
    },
    CommandPending {
        device_ref: DeviceRef,
        request_id: RequestId,
        target: TargetContext,
        display_summary: String,
    },
    TaskSnapshot {
        target: TargetContext,
        snapshot: Box<TaskSnapshot>,
    },
    TaskEvent {
        target: TargetContext,
        event: TaskEvent,
    },
    TaskOutput {
        target: TargetContext,
        chunk: OutputChunk,
        range: OutputRange,
    },
    TaskOutputGap {
        target: TargetContext,
        task_ref: TaskRef,
        stream: OutputStream,
        missing_from: u64,
        missing_to: u64,
    },
    TaskRetrying {
        target: TargetContext,
        request_id: RequestId,
        retry_in_ms: u64,
        message: String,
    },
    TaskStopped {
        target: Option<TargetContext>,
        request_id: RequestId,
        message: String,
    },
}
