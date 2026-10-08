use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{DeviceId, EndpointKey, TenantId, UserId};
pub const RELAY_POLICY_SCHEMA_VERSION: u16 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrafficScope {
    User { user_id: UserId },
    Guest { endpoint_key: EndpointKey },
}

impl TrafficScope {
    pub const fn user_id(self) -> Option<UserId> {
        match self {
            Self::User { user_id } => Some(user_id),
            Self::Guest { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayLimitDefaults {
    pub user_mbps: u32,
    pub guest_mbps: u32,
}

impl RelayLimitDefaults {
    pub fn validate(self) -> Result<Self, LimitConfigError> {
        if self.user_mbps == 0 || self.guest_mbps == 0 {
            return Err(LimitConfigError::ZeroRate);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRelayLimit {
    pub user_id: UserId,
    pub mbps: u32,
}

impl UserRelayLimit {
    pub fn validate(self) -> Result<Self, LimitConfigError> {
        if self.mbps == 0 {
            return Err(LimitConfigError::ZeroRate);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RelayEndpointOwner {
    User {
        scope: TrafficScope,
    },
    Device {
        tenant_id: TenantId,
        device_id: DeviceId,
    },
    Guest,
}

impl RelayEndpointOwner {
    pub const fn tenant_id(self) -> Option<TenantId> {
        match self {
            Self::User { .. } => None,
            Self::Device { tenant_id, .. } => Some(tenant_id),
            Self::Guest => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayConnectionIntent {
    pub operator_endpoint_key: EndpointKey,
    pub device_id: DeviceId,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayEndpointPolicy {
    pub endpoint_key: EndpointKey,
    pub owner: RelayEndpointOwner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayPolicySnapshot {
    pub schema_version: u16,
    pub policy_version: u64,
    pub issued_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub defaults: RelayLimitDefaults,
    pub user_limits: Vec<UserRelayLimit>,
    pub endpoints: Vec<RelayEndpointPolicy>,
    pub connection_intents: Vec<RelayConnectionIntent>,
}

impl RelayPolicySnapshot {
    pub fn validate(&self) -> Result<(), RelayPolicyError> {
        if self.schema_version != RELAY_POLICY_SCHEMA_VERSION {
            return Err(RelayPolicyError::UnsupportedSchemaVersion(
                self.schema_version,
            ));
        }
        if self.policy_version == 0 {
            return Err(RelayPolicyError::ZeroPolicyVersion);
        }
        if self.expires_at_unix_ms <= self.issued_at_unix_ms {
            return Err(RelayPolicyError::InvalidValidityWindow);
        }
        self.defaults.validate()?;

        let mut users = HashSet::with_capacity(self.user_limits.len());
        for limits in &self.user_limits {
            limits.validate()?;
            if !users.insert(limits.user_id) {
                return Err(RelayPolicyError::DuplicateUser(limits.user_id));
            }
        }

        let mut endpoints = HashSet::with_capacity(self.endpoints.len());
        for endpoint in &self.endpoints {
            if !endpoints.insert(endpoint.endpoint_key) {
                return Err(RelayPolicyError::DuplicateEndpoint(endpoint.endpoint_key));
            }
        }
        let mut grants = HashSet::new();
        for intent in &self.connection_intents {
            if !grants.insert((intent.operator_endpoint_key, intent.device_id)) {
                return Err(RelayPolicyError::DuplicateConnectionIntent);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LimitConfigError {
    #[error("relay rate limits must be greater than zero")]
    ZeroRate,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RelayPolicyError {
    #[error("unsupported relay policy schema version {0}")]
    UnsupportedSchemaVersion(u16),
    #[error("relay policy version must be greater than zero")]
    ZeroPolicyVersion,
    #[error("relay policy expiration must be after its issue time")]
    InvalidValidityWindow,
    #[error(transparent)]
    InvalidLimit(#[from] LimitConfigError),
    #[error("relay policy contains duplicate user {0}")]
    DuplicateUser(UserId),
    #[error("relay policy contains a duplicate endpoint {0:?}")]
    DuplicateEndpoint(EndpointKey),
    #[error("relay policy contains a duplicate connection intent")]
    DuplicateConnectionIntent,
}

pub fn mbps_to_bytes_per_second(mbps: u32) -> u64 {
    u64::from(mbps) * 1_000_000 / 8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> RelayLimitDefaults {
        RelayLimitDefaults {
            user_mbps: 5,
            guest_mbps: 1,
        }
    }

    #[test]
    fn converts_decimal_megabits_to_bytes() {
        assert_eq!(mbps_to_bytes_per_second(20), 2_500_000);
    }

    #[test]
    fn rejects_zero_as_an_implicit_unlimited_value() {
        let result = RelayLimitDefaults {
            user_mbps: 0,
            ..defaults()
        }
        .validate();
        assert_eq!(result, Err(LimitConfigError::ZeroRate));
    }

    #[test]
    fn rejects_duplicate_endpoints_in_a_snapshot() {
        let user_id = UserId::new();
        let endpoint = RelayEndpointPolicy {
            endpoint_key: EndpointKey::new([7; 32]),
            owner: RelayEndpointOwner::User {
                scope: TrafficScope::User { user_id },
            },
        };
        let snapshot = RelayPolicySnapshot {
            schema_version: RELAY_POLICY_SCHEMA_VERSION,
            policy_version: 1,
            issued_at_unix_ms: 10,
            expires_at_unix_ms: 20,
            defaults: defaults(),
            user_limits: Vec::new(),
            endpoints: vec![endpoint, endpoint],
            connection_intents: Vec::new(),
        };
        assert_eq!(
            snapshot.validate(),
            Err(RelayPolicyError::DuplicateEndpoint(endpoint.endpoint_key))
        );
    }
}
