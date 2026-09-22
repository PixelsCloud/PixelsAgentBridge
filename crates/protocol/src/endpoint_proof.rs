use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ChallengeId, ConnectionId, DeploymentId, TenantId, UserId};

pub const ENDPOINT_PROOF_SCHEMA_VERSION: u16 = 1;
const ENDPOINT_PROOF_DOMAIN: &str = "pixels-agent-bridge endpoint proof v1";

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
