use serde::{Deserialize, Serialize};

use crate::{RelayPolicySnapshot, RequestId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayControlClientMessage {
    Goodbye {
        request_id: RequestId,
        node_id: String,
        boot_id: uuid::Uuid,
    },
    GetProbe {
        request_id: RequestId,
        node_id: String,
        boot_id: uuid::Uuid,
        applied: Option<uuid::Uuid>,
    },
    Heartbeat {
        request_id: RequestId,
        node_id: String,
        boot_id: uuid::Uuid,
        connections: u64,
        received_bytes: u64,
        sent_bytes: u64,
    },
    GetBudgets {
        request_id: RequestId,
        node_id: String,
        boot_id: uuid::Uuid,
        users: Vec<crate::UserId>,
    },
    Hello {
        request_id: RequestId,
        node_id: String,
        boot_id: uuid::Uuid,
        cluster_schema: u16,
        policy_schema: u16,
        agent_version: String,
    },
    ReportUsage {
        request_id: RequestId,
        node_id: String,
        batch: crate::UsageBatch,
    },
    GetPolicy {
        request_id: RequestId,
        known_policy_version: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_version: Option<String>,
    },
}

impl RelayControlClientMessage {
    pub const fn request_id(&self) -> RequestId {
        match self {
            Self::Goodbye { request_id, .. } => *request_id,
            Self::GetProbe { request_id, .. } => *request_id,
            Self::Hello { request_id, .. }
            | Self::GetBudgets { request_id, .. }
            | Self::Heartbeat { request_id, .. } => *request_id,
            Self::GetPolicy { request_id, .. } | Self::ReportUsage { request_id, .. } => {
                *request_id
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayControlServerMessage {
    GoodbyeAccepted {
        request_id: RequestId,
    },
    Probe {
        request_id: RequestId,
        snapshot: crate::RelayProbeSnapshot,
    },
    HeartbeatAccepted {
        request_id: RequestId,
    },
    Budgets {
        request_id: RequestId,
        snapshot: crate::RelayBudgetSnapshot,
    },
    HelloAccepted {
        request_id: RequestId,
        heartbeat_seconds: u64,
        capacity_bytes_per_second: u64,
    },
    UsageAccepted {
        request_id: RequestId,
    },
    PolicyChanged,
    PolicySnapshot {
        request_id: RequestId,
        snapshot: RelayPolicySnapshot,
    },
    PolicyUnchanged {
        request_id: RequestId,
        policy_version: u64,
        expires_at_unix_ms: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        issued_at_unix_ms: Option<i64>,
    },
    Error {
        request_id: Option<RequestId>,
        code: RelayControlErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayControlErrorCode {
    InvalidMessage,
    InvalidPolicyVersion,
    Internal,
}
