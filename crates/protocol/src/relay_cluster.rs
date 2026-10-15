//! Cluster metadata is carried on authenticated control channels, never on data streams.
use crate::UserId;
use serde::{Deserialize, Serialize};

pub const RELAY_CLUSTER_SCHEMA_VERSION: u16 = 1;
pub const MAX_RELAY_NODES: usize = 64;
pub const MAX_RELAY_BUDGET_DEMANDS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayProbeGrant {
    pub probe_id: uuid::Uuid,
    pub first: crate::EndpointKey,
    pub second: crate::EndpointKey,
    pub expires_at_unix_ms: i64,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayProbeSnapshot {
    pub node_id: String,
    pub boot_id: uuid::Uuid,
    pub issued_at_unix_ms: i64,
    pub grant: Option<RelayProbeGrant>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayAdminState {
    #[default]
    Active,
    Maintenance,
    Disabled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayHealth {
    #[default]
    Starting,
    Healthy,
    Suspect,
    Offline,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayDirectoryNode {
    pub node_id: String,
    pub url: String,
    pub region: String,
    pub weight: u32,
    pub capacity_mbps: u32,
    pub admin_state: RelayAdminState,
    pub health: RelayHealth,
}

impl RelayDirectoryNode {
    pub fn accepts_new_connections(&self) -> bool {
        self.admin_state == RelayAdminState::Active && self.health == RelayHealth::Healthy
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayDirectory {
    pub schema_version: u16,
    pub revision: u64,
    pub nodes: Vec<RelayDirectoryNode>,
}

impl RelayDirectory {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != RELAY_CLUSTER_SCHEMA_VERSION || self.revision == 0 {
            return Err("unsupported Relay directory version");
        }
        if self.nodes.len() > MAX_RELAY_NODES {
            return Err("too many Relay nodes");
        }
        let mut ids = std::collections::HashSet::new();
        let mut urls = std::collections::HashSet::new();
        for node in &self.nodes {
            if !valid_relay_node_id(&node.node_id)
                || node.weight == 0
                || node.weight > 1000
                || node.capacity_mbps == 0
                || node.region.len() > 64
                || node.region.chars().any(char::is_control)
                || node.url.len() > 2048
                || !ids.insert(&node.node_id)
                || !urls.insert(&node.url)
            {
                return Err("invalid or duplicate Relay directory entry");
            }
        }
        Ok(())
    }
}

pub fn valid_relay_node_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// A grant belongs to exactly one Relay process and account. Auth policy is separate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayBudgetGrant {
    pub user_id: UserId,
    pub generation: u64,
    pub bytes_per_second: u64,
    pub burst_bytes: u64,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayBudgetSnapshot {
    pub schema_version: u16,
    pub node_id: String,
    pub boot_id: uuid::Uuid,
    /// Revision of the entire snapshot, including empty/revocation snapshots.
    pub revision: u64,
    pub issued_at_unix_ms: i64,
    pub node_capacity_bytes_per_second: u64,
    /// Grants replace only these accounts, allowing bounded control batches.
    pub requested_users: Vec<UserId>,
    pub grants: Vec<RelayBudgetGrant>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_is_bounded_and_health_does_not_override_maintenance() {
        let node = RelayDirectoryNode {
            node_id: "a".into(),
            url: "https://a.test/".into(),
            region: "test".into(),
            weight: 1,
            capacity_mbps: 100,
            admin_state: RelayAdminState::Maintenance,
            health: RelayHealth::Healthy,
        };
        assert!(!node.accepts_new_connections());
        let mut directory = RelayDirectory {
            schema_version: 1,
            revision: 1,
            nodes: vec![node.clone()],
        };
        assert!(directory.validate().is_ok());
        directory.nodes.push(node);
        assert!(directory.validate().is_err());
        directory.nodes.clear();
        assert!(directory.validate().is_ok()); // All nodes offline is a state, not malformed input.
        directory.revision = 0;
        assert!(directory.validate().is_err());
    }
    #[test]
    fn node_identity_rejects_paths_and_controls() {
        for value in ["", "../a", "a/b", "a b", "a\n", "你好"] {
            assert!(!valid_relay_node_id(value));
        }
        assert!(valid_relay_node_id("cn-1.example_2"));
    }
}
