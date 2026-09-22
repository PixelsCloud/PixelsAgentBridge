#![forbid(unsafe_code)]

use std::{collections::HashSet, fmt};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const RELAY_POLICY_SCHEMA_VERSION: u16 = 1;
pub const ENDPOINT_PROOF_SCHEMA_VERSION: u16 = 1;
const ENDPOINT_PROOF_DOMAIN: &str = "pixels-agent-bridge endpoint proof v1";

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            pub const fn from_u128(value: u128) -> Self {
                Self(Uuid::from_u128(value))
            }

            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self::from_uuid(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.as_uuid()
            }
        }
    };
}

uuid_id!(DeploymentId);
uuid_id!(UserId);
uuid_id!(TenantId);
uuid_id!(DeviceId);
uuid_id!(ConnectionId);
uuid_id!(ChallengeId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EndpointKey([u8; 32]);

impl EndpointKey {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub const fn into_bytes(self) -> [u8; 32] {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EndpointSignature([u8; 32], [u8; 32]);

impl EndpointSignature {
    pub fn from_bytes(bytes: [u8; 64]) -> Self {
        let mut first = [0; 32];
        let mut second = [0; 32];
        first.copy_from_slice(&bytes[..32]);
        second.copy_from_slice(&bytes[32..]);
        Self(first, second)
    }

    pub fn to_bytes(self) -> [u8; 64] {
        let mut bytes = [0; 64];
        bytes[..32].copy_from_slice(&self.0);
        bytes[32..].copy_from_slice(&self.1);
        bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointProofPurpose {
    RegisterUserEndpoint,
    RegisterDevice,
    AuthenticateRegisteredEndpoint,
}

impl EndpointProofPurpose {
    const fn tag(self) -> u8 {
        match self {
            Self::RegisterUserEndpoint => 1,
            Self::RegisterDevice => 2,
            Self::AuthenticateRegisteredEndpoint => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointProofChallenge {
    pub schema_version: u16,
    pub challenge_id: ChallengeId,
    pub deployment_id: DeploymentId,
    pub connection_id: ConnectionId,
    pub user_id: UserId,
    pub tenant_id: TenantId,
    pub endpoint_key: EndpointKey,
    pub purpose: EndpointProofPurpose,
    pub issued_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub nonce: [u8; 32],
}

impl EndpointProofChallenge {
    pub fn signing_message(&self) -> [u8; 32] {
        let mut context = Vec::with_capacity(163);
        context.extend_from_slice(&self.schema_version.to_be_bytes());
        context.extend_from_slice(self.challenge_id.as_uuid().as_bytes());
        context.extend_from_slice(self.deployment_id.as_uuid().as_bytes());
        context.extend_from_slice(self.connection_id.as_uuid().as_bytes());
        context.extend_from_slice(self.user_id.as_uuid().as_bytes());
        context.extend_from_slice(self.tenant_id.as_uuid().as_bytes());
        context.extend_from_slice(self.endpoint_key.as_bytes());
        context.push(self.purpose.tag());
        context.extend_from_slice(&self.issued_at_unix_ms.to_be_bytes());
        context.extend_from_slice(&self.expires_at_unix_ms.to_be_bytes());
        context.extend_from_slice(&self.nonce);
        blake3::derive_key(ENDPOINT_PROOF_DOMAIN, &context)
    }

    pub fn validate_at(&self, unix_ms: i64) -> Result<(), EndpointProofContractError> {
        if self.schema_version != ENDPOINT_PROOF_SCHEMA_VERSION {
            return Err(EndpointProofContractError::UnsupportedSchemaVersion(
                self.schema_version,
            ));
        }
        if self.expires_at_unix_ms <= self.issued_at_unix_ms {
            return Err(EndpointProofContractError::InvalidValidityWindow);
        }
        if unix_ms < self.issued_at_unix_ms {
            return Err(EndpointProofContractError::NotYetValid);
        }
        if unix_ms >= self.expires_at_unix_ms {
            return Err(EndpointProofContractError::Expired);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointProofResponse {
    pub challenge_id: ChallengeId,
    pub signature: EndpointSignature,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EndpointProofContractError {
    #[error("unsupported endpoint proof schema version {0}")]
    UnsupportedSchemaVersion(u16),
    #[error("endpoint proof expiration must be after its issue time")]
    InvalidValidityWindow,
    #[error("endpoint proof challenge is not valid yet")]
    NotYetValid,
    #[error("endpoint proof challenge has expired")]
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrafficScope {
    Team {
        tenant_id: TenantId,
        user_id: UserId,
    },
    Personal {
        tenant_id: TenantId,
        user_id: UserId,
    },
}

impl TrafficScope {
    pub const fn tenant_id(self) -> TenantId {
        match self {
            Self::Team { tenant_id, .. } | Self::Personal { tenant_id, .. } => tenant_id,
        }
    }

    pub const fn user_id(self) -> UserId {
        match self {
            Self::Team { user_id, .. } | Self::Personal { user_id, .. } => user_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayLimitDefaults {
    pub team_mbps: u32,
    pub member_mbps: u32,
    pub personal_mbps: u32,
}

impl RelayLimitDefaults {
    pub fn validate(self) -> Result<Self, LimitConfigError> {
        if self.team_mbps == 0 || self.member_mbps == 0 || self.personal_mbps == 0 {
            return Err(LimitConfigError::ZeroRate);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamRelayLimits {
    pub tenant_id: TenantId,
    pub total_mbps: u32,
    pub member_mbps: u32,
}

impl TeamRelayLimits {
    pub fn validate(self) -> Result<Self, LimitConfigError> {
        if self.total_mbps == 0 || self.member_mbps == 0 {
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
}

impl RelayEndpointOwner {
    pub const fn tenant_id(self) -> TenantId {
        match self {
            Self::User { scope } => scope.tenant_id(),
            Self::Device { tenant_id, .. } => tenant_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayEndpointPolicy {
    pub endpoint_key: EndpointKey,
    pub owner: RelayEndpointOwner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayPolicySnapshot {
    pub schema_version: u16,
    pub deployment_id: DeploymentId,
    pub policy_version: u64,
    pub issued_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub defaults: RelayLimitDefaults,
    pub team_limits: Vec<TeamRelayLimits>,
    pub endpoints: Vec<RelayEndpointPolicy>,
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

        let mut tenants = HashSet::with_capacity(self.team_limits.len());
        for limits in &self.team_limits {
            limits.validate()?;
            if !tenants.insert(limits.tenant_id) {
                return Err(RelayPolicyError::DuplicateTeam(limits.tenant_id));
            }
        }

        let mut endpoints = HashSet::with_capacity(self.endpoints.len());
        for endpoint in &self.endpoints {
            if !endpoints.insert(endpoint.endpoint_key) {
                return Err(RelayPolicyError::DuplicateEndpoint(endpoint.endpoint_key));
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
    #[error("relay policy contains duplicate Team {0}")]
    DuplicateTeam(TenantId),
    #[error("relay policy contains a duplicate endpoint {0:?}")]
    DuplicateEndpoint(EndpointKey),
}

pub fn mbps_to_bytes_per_second(mbps: u32) -> u64 {
    u64::from(mbps) * 1_000_000 / 8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> RelayLimitDefaults {
        RelayLimitDefaults {
            team_mbps: 20,
            member_mbps: 4,
            personal_mbps: 5,
        }
    }

    #[test]
    fn converts_decimal_megabits_to_bytes() {
        assert_eq!(mbps_to_bytes_per_second(20), 2_500_000);
    }

    #[test]
    fn rejects_zero_as_an_implicit_unlimited_value() {
        let result = RelayLimitDefaults {
            member_mbps: 0,
            ..defaults()
        }
        .validate();
        assert_eq!(result, Err(LimitConfigError::ZeroRate));
    }

    #[test]
    fn rejects_duplicate_endpoints_in_a_snapshot() {
        let user_id = UserId::new();
        let tenant_id = TenantId::new();
        let endpoint = RelayEndpointPolicy {
            endpoint_key: EndpointKey::new([7; 32]),
            owner: RelayEndpointOwner::User {
                scope: TrafficScope::Personal { tenant_id, user_id },
            },
        };
        let snapshot = RelayPolicySnapshot {
            schema_version: RELAY_POLICY_SCHEMA_VERSION,
            deployment_id: DeploymentId::new(),
            policy_version: 1,
            issued_at_unix_ms: 10,
            expires_at_unix_ms: 20,
            defaults: defaults(),
            team_limits: Vec::new(),
            endpoints: vec![endpoint, endpoint],
        };

        assert_eq!(
            snapshot.validate(),
            Err(RelayPolicyError::DuplicateEndpoint(endpoint.endpoint_key))
        );
    }
}
