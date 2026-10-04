use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ChallengeId, ConnectionId, DeviceId, TenantId, UserId};
pub const ENDPOINT_PROOF_SCHEMA_VERSION: u16 = 3;
pub const ENDPOINT_PROOF_CLOCK_SKEW_MS: i64 = 30_000;
const ENDPOINT_PROOF_DOMAIN: &str = "pixels-agent-bridge endpoint proof v3";

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
    RegisterUnclaimedDevice,
    RegisterGuestEndpoint,
}

impl EndpointProofPurpose {
    const fn tag(self) -> u8 {
        match self {
            Self::RegisterUserEndpoint => 1,
            Self::RegisterDevice => 2,
            Self::AuthenticateRegisteredEndpoint => 3,
            Self::RegisterUnclaimedDevice => 4,
            Self::RegisterGuestEndpoint => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EndpointProofPrincipal {
    User { user_id: UserId },
    Device { device_id: DeviceId },
    Guest,
}

impl EndpointProofPrincipal {
    fn encode(self, context: &mut Vec<u8>) {
        match self {
            Self::User { user_id } => {
                context.push(1);
                context.extend_from_slice(user_id.as_uuid().as_bytes());
            }
            Self::Device { device_id } => {
                context.push(2);
                context.extend_from_slice(device_id.as_uuid().as_bytes());
            }
            Self::Guest => context.push(3),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointProofChallenge {
    pub schema_version: u16,
    pub challenge_id: ChallengeId,
    pub connection_id: ConnectionId,
    pub principal: EndpointProofPrincipal,
    pub tenant_id: TenantId,
    pub endpoint_key: EndpointKey,
    pub purpose: EndpointProofPurpose,
    pub issued_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub nonce: [u8; 32],
}

impl EndpointProofChallenge {
    pub fn signing_message(&self) -> [u8; 32] {
        let mut context = Vec::with_capacity(164);
        context.extend_from_slice(&self.schema_version.to_be_bytes());
        context.extend_from_slice(self.challenge_id.as_uuid().as_bytes());
        context.extend_from_slice(self.connection_id.as_uuid().as_bytes());
        self.principal.encode(&mut context);
        context.extend_from_slice(self.tenant_id.as_uuid().as_bytes());
        context.extend_from_slice(self.endpoint_key.as_bytes());
        context.push(self.purpose.tag());
        context.extend_from_slice(&self.issued_at_unix_ms.to_be_bytes());
        context.extend_from_slice(&self.expires_at_unix_ms.to_be_bytes());
        context.extend_from_slice(&self.nonce);
        blake3::derive_key(ENDPOINT_PROOF_DOMAIN, &context)
    }

    pub fn validate_at(&self, unix_ms: i64) -> Result<(), EndpointProofContractError> {
        self.validate_at_with_skew(unix_ms, 0)
    }

    pub fn validate_at_with_skew(
        &self,
        unix_ms: i64,
        allowed_skew_ms: i64,
    ) -> Result<(), EndpointProofContractError> {
        if self.schema_version != ENDPOINT_PROOF_SCHEMA_VERSION {
            return Err(EndpointProofContractError::UnsupportedSchemaVersion(
                self.schema_version,
            ));
        }
        if self.expires_at_unix_ms <= self.issued_at_unix_ms {
            return Err(EndpointProofContractError::InvalidValidityWindow);
        }
        if allowed_skew_ms < 0 {
            return Err(EndpointProofContractError::InvalidClockSkew);
        }
        if unix_ms.saturating_add(allowed_skew_ms) < self.issued_at_unix_ms {
            return Err(EndpointProofContractError::NotYetValid);
        }
        if unix_ms >= self.expires_at_unix_ms.saturating_add(allowed_skew_ms) {
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
    #[error("allowed clock skew must not be negative")]
    InvalidClockSkew,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn challenge(principal: EndpointProofPrincipal) -> EndpointProofChallenge {
        EndpointProofChallenge {
            schema_version: ENDPOINT_PROOF_SCHEMA_VERSION,
            challenge_id: ChallengeId::from_u128(1),
            connection_id: ConnectionId::from_u128(3),
            principal,
            tenant_id: TenantId::from_u128(4),
            endpoint_key: EndpointKey::new([5; 32]),
            purpose: EndpointProofPurpose::AuthenticateRegisteredEndpoint,
            issued_at_unix_ms: 1_000,
            expires_at_unix_ms: 2_000,
            nonce: [6; 32],
        }
    }

    #[test]
    fn old_deployment_bound_schema_is_rejected() {
        let mut challenge = challenge(EndpointProofPrincipal::Guest);
        challenge.schema_version = 2;
        assert_eq!(
            challenge.validate_at(1500),
            Err(EndpointProofContractError::UnsupportedSchemaVersion(2))
        );
    }

    #[test]
    fn signing_message_binds_the_typed_principal() {
        let user = challenge(EndpointProofPrincipal::User {
            user_id: UserId::from_u128(7),
        });
        let device = challenge(EndpointProofPrincipal::Device {
            device_id: DeviceId::from_u128(7),
        });

        assert_ne!(user.signing_message(), device.signing_message());
    }

    #[test]
    fn validation_allows_only_the_configured_clock_skew() {
        let challenge = challenge(EndpointProofPrincipal::User {
            user_id: UserId::from_u128(7),
        });

        assert_eq!(
            challenge.validate_at_with_skew(970, 29),
            Err(EndpointProofContractError::NotYetValid)
        );
        assert_eq!(challenge.validate_at_with_skew(970, 30), Ok(()));
        assert_eq!(challenge.validate_at_with_skew(2_029, 30), Ok(()));
        assert_eq!(
            challenge.validate_at_with_skew(2_030, 30),
            Err(EndpointProofContractError::Expired)
        );
        assert_eq!(
            challenge.validate_at_with_skew(1_500, -1),
            Err(EndpointProofContractError::InvalidClockSkew)
        );
    }
}
