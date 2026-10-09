use crate::{DeviceId, EndpointKey, EndpointSignature, UserId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Automatic never reverses a previous unlink. Replace requires an explicit local action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceAccountAction {
    Automatic,
    Associate,
    Replace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceAccountChallengeRequest {
    pub action: DeviceAccountAction,
    /// Explicit actions compare the state displayed to the user.
    pub expected_revision: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceAccountChallenge {
    pub id: Uuid,
    pub server_origin: String,
    pub device_id: DeviceId,
    pub endpoint_key: EndpointKey,
    pub user_id: UserId,
    pub action: DeviceAccountAction,
    pub expected_revision: i64,
    pub expires_at_unix_ms: i64,
}

impl DeviceAccountChallenge {
    pub fn signing_message(&self) -> Vec<u8> {
        serde_json::to_vec(&("pab.device-account.v1", self)).expect("fixed JSON contract")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceAccountProof {
    pub challenge_id: Uuid,
    pub signature: EndpointSignature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceAccountStatus {
    Available,
    Associated,
    OtherAccount,
    Unlinked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceAccountState {
    pub status: DeviceAccountStatus,
    pub revision: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenge: Option<DeviceAccountChallenge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceAccountUnlink {
    pub revision: i64,
}
