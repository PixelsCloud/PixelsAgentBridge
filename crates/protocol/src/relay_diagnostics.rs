//! Bounded, observational Relay telemetry. These are endpoint pairs, not the
//! encrypted application sessions multiplexed over those pairs.
use crate::EndpointKey;
use serde::{Deserialize, Serialize};

pub const RELAY_DIAGNOSTICS_VERSION: u16 = 1;
pub const RELAY_DIAGNOSTICS_CONNECTION_LIMIT: usize = 1024;
pub const RELAY_DIAGNOSTICS_LINK_LIMIT: usize = 2048;
pub const RELAY_LINK_ACTIVE_MS: u64 = 15_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayHostSnapshot {
    pub hostname: Option<String>,
    pub os: String,
    pub os_version: Option<String>,
    pub architecture: String,
    pub cpu_model: Option<String>,
    pub logical_cpus: u32,
    pub cpu_usage_basis_points: Option<u16>,
    pub memory_total: u64,
    pub memory_used: u64,
    pub host_uptime_ms: u64,
    pub process_uptime_ms: u64,
    pub process_memory: Option<u64>,
    pub containerized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayConnectionSnapshot {
    pub id: String,
    pub endpoint: EndpointKey,
    pub probe: bool,
    pub age_ms: u64,
    pub connected_ms: u64,
    pub closed_ago_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayLinkSnapshot {
    pub id: uuid::Uuid,
    pub a: EndpointKey,
    pub b: EndpointKey,
    pub age_ms: u64,
    pub observed_ms: u64,
    pub last_forwarded_ago_ms: u64,
    pub closed_ago_ms: Option<u64>,
    pub a_to_b_bytes: u64,
    pub b_to_a_bytes: u64,
    pub a_to_b_packets: u64,
    pub b_to_a_packets: u64,
    pub a_to_b_bytes_per_second: u64,
    pub b_to_a_bytes_per_second: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayDiagnosticsSnapshot {
    pub version: u16,
    pub host: RelayHostSnapshot,
    pub traffic: RelayTrafficSnapshot,
    pub connections_total: u64,
    pub links_total: u64,
    pub active_links: u64,
    pub connections_truncated: bool,
    pub links_truncated: bool,
    pub connections: Vec<RelayConnectionSnapshot>,
    pub links: Vec<RelayLinkSnapshot>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayTrafficSnapshot {
    pub accepted_connections: u64,
    pub disconnected_connections: u64,
    pub received_bytes: u64,
    pub sent_bytes: u64,
    pub received_packets: u64,
    pub sent_packets: u64,
    pub dropped_packets: u64,
    pub rate_limited_bytes: u64,
    pub ping_frames: u64,
    pub received_bytes_per_second: u64,
    pub sent_bytes_per_second: u64,
}

impl RelayDiagnosticsSnapshot {
    pub fn valid(&self) -> bool {
        let text = |value: &str| value.len() <= 256 && !value.chars().any(char::is_control);
        let duration = |value: u64| value <= 10 * 366 * 86_400_000;
        self.version == RELAY_DIAGNOSTICS_VERSION
            && self.connections.len() <= RELAY_DIAGNOSTICS_CONNECTION_LIMIT
            && self.links.len() <= RELAY_DIAGNOSTICS_LINK_LIMIT
            && self.host.hostname.as_deref().is_none_or(text)
            && text(&self.host.os)
            && text(&self.host.architecture)
            && self.host.os_version.as_deref().is_none_or(text)
            && self.host.cpu_model.as_deref().is_none_or(text)
            && self.host.cpu_usage_basis_points.is_none_or(|n| n <= 10_000)
            && self.host.memory_used <= self.host.memory_total
            && self.active_links <= self.links_total
            && self.connections.iter().all(|r| {
                !r.id.is_empty()
                    && r.id.len() <= 24
                    && r.id.bytes().all(|c| c.is_ascii_digit())
                    && duration(r.age_ms)
                    && r.connected_ms <= r.age_ms
                    && r.closed_ago_ms.is_none_or(|age| age <= r.age_ms)
            })
            && self.links.iter().all(|r| {
                !r.id.is_nil()
                    && r.a != r.b
                    && duration(r.age_ms)
                    && r.observed_ms <= r.age_ms
                    && r.last_forwarded_ago_ms <= r.age_ms
                    && r.closed_ago_ms.is_none_or(|age| age <= r.age_ms)
            })
    }
}
