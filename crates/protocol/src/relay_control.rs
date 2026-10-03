use serde::{Deserialize, Serialize};

use crate::{DeploymentId, RelayPolicySnapshot, RequestId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayControlClientMessage {
    GetPolicy {
        request_id: RequestId,
        deployment_id: DeploymentId,
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
            Self::GetPolicy { request_id, .. } => *request_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayControlServerMessage {
    PolicySnapshot {
        request_id: RequestId,
        snapshot: RelayPolicySnapshot,
    },
    PolicyUnchanged {
        request_id: RequestId,
        deployment_id: DeploymentId,
        policy_version: u64,
        expires_at_unix_ms: i64,
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
    InvalidDeployment,
    InvalidPolicyVersion,
    Internal,
}
