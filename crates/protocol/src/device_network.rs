use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::{DeviceRef, EndpointInstanceId, EndpointKey};

pub const DEVICE_NETWORK_SCHEMA_VERSION: u16 = 1;
pub const MAX_DEVICE_RELAY_URLS: usize = 8;
pub const MAX_DEVICE_DIRECT_ADDRESSES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceNetworkUpdate {
    pub schema_version: u16,
    pub device_ref: DeviceRef,
    pub endpoint_key: EndpointKey,
    pub endpoint_instance_id: EndpointInstanceId,
    pub address_revision: u64,
    pub relay_urls: Vec<String>,
    pub direct_addresses: Vec<SocketAddr>,
    pub observed_at_unix_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceNetworkResult {
    pub device_ref: DeviceRef,
    pub endpoint_instance_id: EndpointInstanceId,
    pub address_revision: u64,
    pub accepted_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceNetworkSnapshot {
    pub schema_version: u16,
    pub device_ref: DeviceRef,
    pub endpoint_key: EndpointKey,
    pub endpoint_instance_id: EndpointInstanceId,
    pub address_revision: u64,
    pub relay_urls: Vec<String>,
    pub direct_addresses: Vec<SocketAddr>,
    pub observed_at_unix_ms: i64,
    pub accepted_at_unix_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeploymentId, DeviceId, TenantId};

    #[test]
    fn network_update_round_trips_without_iroh_types() {
        let update = DeviceNetworkUpdate {
            schema_version: DEVICE_NETWORK_SCHEMA_VERSION,
            device_ref: DeviceRef {
                deployment_id: DeploymentId::from_u128(1),
                tenant_id: TenantId::from_u128(2),
                device_id: DeviceId::from_u128(3),
            },
            endpoint_key: EndpointKey::new([4; 32]),
            endpoint_instance_id: EndpointInstanceId::from_u128(5),
            address_revision: 6,
            relay_urls: vec!["https://relay.example".to_owned()],
            direct_addresses: vec!["192.0.2.1:7842".parse().unwrap()],
            observed_at_unix_ms: 7,
        };

        let encoded = serde_json::to_string(&update).unwrap();
        assert_eq!(
            serde_json::from_str::<DeviceNetworkUpdate>(&encoded).unwrap(),
            update
        );
    }
}
