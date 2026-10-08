use crate::{EndpointKey, EndpointSignature, UserId};
use serde::{Deserialize, Serialize};

/// Supplementary user attribution. OperatorRef still owns tasks and transfers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserAttribution {
    pub user_id: UserId,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointUserContextUpdate {
    pub endpoint_key: EndpointKey,
    pub revision: u64,
    pub issued_at_unix_ms: i64,
    pub signature: EndpointSignature,
}

impl EndpointUserContextUpdate {
    pub fn signing_message(&self, server_origin: &str, session_digest: &str) -> Vec<u8> {
        // JSON tuple framing prevents ambiguous origin/digest concatenation.
        serde_json::to_vec(&(
            "pab.endpoint-user-context.v1",
            server_origin,
            session_digest,
            self.endpoint_key,
            self.revision,
            self.issued_at_unix_ms,
        ))
        .expect("fixed JSON tuple")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointUserContext {
    pub revision: u64,
    pub user: Option<UserAttribution>,
    pub policy_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayUserContextReceipt {
    pub node_id: String,
    pub applied_policy_version: Option<u64>,
    pub online: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointUserContextReceipt {
    #[serde(flatten)]
    pub context: EndpointUserContext,
    pub relay_nodes: Vec<RelayUserContextReceipt>,
}
