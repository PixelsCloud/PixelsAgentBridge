//! Relay cluster configuration. Secrets stay server-side; directory DTOs are separate.
use pab_protocol::{MAX_RELAY_NODES, valid_relay_node_id};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayClusterConfig {
    #[serde(default = "heartbeat_seconds")]
    pub heartbeat_seconds: u64,
    #[serde(default = "offline_seconds")]
    pub offline_seconds: u64,
    #[serde(default = "recovery_seconds")]
    pub recovery_seconds: u64,
    #[serde(default = "maintenance_seconds")]
    pub maintenance_seconds: u64,
    pub nodes: Vec<RelayNodeConfig>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayNodeConfig {
    pub node_id: String,
    pub url: String,
    #[serde(default)]
    pub region: String,
    #[serde(default = "weight")]
    pub weight: u32,
    pub capacity_mbps: u32,
    pub control_secret: String,
}
fn heartbeat_seconds() -> u64 {
    5
}
fn offline_seconds() -> u64 {
    15
}
fn recovery_seconds() -> u64 {
    10
}
fn maintenance_seconds() -> u64 {
    30
}
fn weight() -> u32 {
    100
}

impl std::fmt::Debug for RelayClusterConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayClusterConfig")
            .field("node_count", &self.nodes.len())
            .finish_non_exhaustive()
    }
}

impl RelayClusterConfig {
    pub fn validate(&mut self) -> Result<(), &'static str> {
        if self.nodes.is_empty() || self.nodes.len() > MAX_RELAY_NODES {
            return Err("relay.cluster requires 1-64 nodes");
        }
        if !(1..=30).contains(&self.heartbeat_seconds)
            || self.offline_seconds < self.heartbeat_seconds.saturating_mul(2)
            || self.offline_seconds > 120
            || self.recovery_seconds > 120
            || self.maintenance_seconds > 300
        {
            return Err("invalid relay.cluster timing settings");
        }
        let mut ids = std::collections::HashSet::new();
        let mut urls = std::collections::HashSet::new();
        let mut secrets = std::collections::HashSet::new();
        for node in &mut self.nodes {
            if !valid_relay_node_id(&node.node_id)
                || !ids.insert(node.node_id.clone())
                || node.weight == 0
                || node.weight > 1000
                || node.capacity_mbps == 0
                || node.capacity_mbps > i32::MAX as u32
                || node.region.len() > 64
                || node.region.chars().any(char::is_control)
            {
                return Err("invalid or duplicate relay.cluster node");
            }
            let parsed = url::Url::parse(&node.url).map_err(|_| "invalid Relay public URL")?;
            if parsed.scheme() != "https"
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || parsed.path() != "/"
                || node.url.len() > 2048
            {
                return Err("Relay public URL requires an HTTPS origin without credentials");
            }
            node.url = parsed.to_string();
            if !urls.insert(node.url.clone()) {
                return Err("duplicate Relay public URL");
            }
            if node.control_secret.len() < 32
                || !node.control_secret.bytes().all(|b| b.is_ascii_graphic())
                || !secrets.insert(node.control_secret.clone())
            {
                return Err(
                    "Relay nodes require distinct control secrets of at least 32 printable ASCII bytes",
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cluster() -> RelayClusterConfig {
        RelayClusterConfig {
            heartbeat_seconds: 5,
            offline_seconds: 15,
            recovery_seconds: 10,
            maintenance_seconds: 30,
            nodes: vec![RelayNodeConfig {
                node_id: "win-90".into(),
                url: "https://relay.example".into(),
                region: "test".into(),
                weight: 100,
                capacity_mbps: 100,
                control_secret: "private-credential-must-never-appear-in-errors".into(),
            }],
        }
    }
    #[test]
    fn validates_entire_catalog_without_exposing_secrets() {
        let mut good = cluster();
        good.validate().unwrap();
        assert_eq!(good.nodes[0].url, "https://relay.example/");
        for url in [
            "http://relay.test",
            "https://user:password@relay.test",
            "https://relay.test/path",
            "https://relay.test/?secret=a",
        ] {
            let mut bad = cluster();
            bad.nodes[0].url = url.into();
            assert!(bad.validate().is_err());
        }
        let mut duplicate = cluster();
        duplicate.nodes.push(duplicate.nodes[0].clone());
        let error = duplicate.validate().unwrap_err();
        assert!(!error.contains("private-credential"));
        let mut bad = cluster();
        bad.offline_seconds = 5;
        assert!(bad.validate().is_err());
    }
}
