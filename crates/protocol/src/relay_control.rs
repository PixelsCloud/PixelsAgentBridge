use serde::{Deserialize, Serialize};

use crate::{RelayPolicySnapshot, RequestId};

/// Process counters sampled by a single-node Relay. Missing data remains unknown
/// when an older Relay or Server is used during a rolling upgrade.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayNodeTelemetry {
    pub boot_id: uuid::Uuid,
    pub public_url: Option<String>,
    pub connections: u64,
    pub received_bytes: u64,
    pub sent_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<crate::RelayDiagnosticsSnapshot>,
}

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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diagnostics: Option<crate::RelayDiagnosticsSnapshot>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        telemetry: Option<RelayNodeTelemetry>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diagnostics_version: Option<u16>,
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

#[cfg(test)]
mod telemetry_tests {
    use super::*;

    #[test]
    fn old_policy_requests_remain_valid_and_omit_missing_telemetry() {
        let old = serde_json::json!({"type":"get_policy","request_id":RequestId::new(),"known_policy_version":null,"node_id":"primary","agent_version":"old"});
        let request: RelayControlClientMessage = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), old);
    }

    #[test]
    fn telemetry_round_trips_full_width_counters() {
        let value = RelayNodeTelemetry {
            boot_id: uuid::Uuid::new_v4(),
            public_url: None,
            connections: 0,
            received_bytes: u64::MAX,
            sent_bytes: 9_007_199_254_740_993,
            diagnostics: None,
        };
        assert_eq!(
            serde_json::from_str::<RelayNodeTelemetry>(&serde_json::to_string(&value).unwrap())
                .unwrap(),
            value
        );
    }
}
