use std::time::Duration;

use iroh_base::{PublicKey, Signature};
use pab_protocol::{
    ChallengeId, ConnectionId, DeploymentId, ENDPOINT_PROOF_SCHEMA_VERSION, EndpointKey,
    EndpointProofChallenge, EndpointProofContractError, EndpointProofPrincipal,
    EndpointProofPurpose, EndpointProofResponse, TenantId,
};
use rand_core::{OsRng, RngCore};
use thiserror::Error;
use time::OffsetDateTime;

const MAX_CHALLENGE_VALIDITY: Duration = Duration::from_secs(120);

#[derive(Debug)]
pub struct EndpointProofSession {
    deployment_id: DeploymentId,
    connection_id: ConnectionId,
    pending: Option<EndpointProofChallenge>,
}

impl EndpointProofSession {
    pub fn new(deployment_id: DeploymentId) -> Self {
        Self {
            deployment_id,
            connection_id: ConnectionId::new(),
            pending: None,
        }
    }

    pub const fn connection_id(&self) -> ConnectionId {
        self.connection_id
    }

    pub fn issue(
        &mut self,
        principal: EndpointProofPrincipal,
        tenant_id: TenantId,
        endpoint_key: EndpointKey,
        purpose: EndpointProofPurpose,
        now: OffsetDateTime,
        validity: Duration,
    ) -> Result<EndpointProofChallenge, EndpointProofError> {
        if validity.is_zero() || validity > MAX_CHALLENGE_VALIDITY {
            return Err(EndpointProofError::InvalidValidity);
        }
        PublicKey::from_bytes(endpoint_key.as_bytes())
            .map_err(|_| EndpointProofError::InvalidEndpointKey)?;
        let expires_at = now
            .checked_add(
                time::Duration::try_from(validity)
                    .map_err(|_| EndpointProofError::InvalidValidity)?,
            )
            .ok_or(EndpointProofError::InvalidValidity)?;
        let mut nonce = [0; 32];
        OsRng.fill_bytes(&mut nonce);
        let challenge = EndpointProofChallenge {
            schema_version: ENDPOINT_PROOF_SCHEMA_VERSION,
            challenge_id: ChallengeId::new(),
            deployment_id: self.deployment_id,
            connection_id: self.connection_id,
            principal,
            tenant_id,
            endpoint_key,
            purpose,
            issued_at_unix_ms: unix_millis(now)?,
            expires_at_unix_ms: unix_millis(expires_at)?,
            nonce,
        };
        challenge.validate_at(challenge.issued_at_unix_ms)?;
        self.pending = Some(challenge.clone());
        Ok(challenge)
    }

    pub fn verify(
        &mut self,
        response: EndpointProofResponse,
        now: OffsetDateTime,
    ) -> Result<VerifiedEndpointProof, EndpointProofError> {
        let challenge = self
            .pending
            .take()
            .ok_or(EndpointProofError::NoPendingChallenge)?;
        if response.challenge_id != challenge.challenge_id {
            return Err(EndpointProofError::ChallengeMismatch);
        }
        challenge.validate_at(unix_millis(now)?)?;
        let public_key = PublicKey::from_bytes(challenge.endpoint_key.as_bytes())
            .map_err(|_| EndpointProofError::InvalidEndpointKey)?;
        let signature = Signature::from_bytes(&response.signature.to_bytes());
        public_key
            .verify(&challenge.signing_message(), &signature)
            .map_err(|_| EndpointProofError::InvalidSignature)?;
        Ok(VerifiedEndpointProof { challenge })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedEndpointProof {
    challenge: EndpointProofChallenge,
}

impl VerifiedEndpointProof {
    pub const fn principal(&self) -> EndpointProofPrincipal {
        self.challenge.principal
    }

    pub const fn tenant_id(&self) -> TenantId {
        self.challenge.tenant_id
    }

    pub const fn endpoint_key(&self) -> EndpointKey {
        self.challenge.endpoint_key
    }

    pub const fn purpose(&self) -> EndpointProofPurpose {
        self.challenge.purpose
    }

    pub const fn connection_id(&self) -> ConnectionId {
        self.challenge.connection_id
    }
}

fn unix_millis(value: OffsetDateTime) -> Result<i64, EndpointProofError> {
    i64::try_from(value.unix_timestamp_nanos() / 1_000_000)
        .map_err(|_| EndpointProofError::InvalidTime)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EndpointProofError {
    #[error("endpoint proof validity must be between 1 ms and 120 seconds")]
    InvalidValidity,
    #[error("endpoint public key is invalid")]
    InvalidEndpointKey,
    #[error("no endpoint proof challenge is pending on this connection")]
    NoPendingChallenge,
    #[error("endpoint proof response does not match the pending challenge")]
    ChallengeMismatch,
    #[error("endpoint proof signature is invalid")]
    InvalidSignature,
    #[error("endpoint proof timestamp is outside the supported range")]
    InvalidTime,
    #[error(transparent)]
    Contract(#[from] EndpointProofContractError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_base::SecretKey;
    use pab_protocol::{EndpointSignature, UserId};

    fn endpoint(secret: &SecretKey) -> EndpointKey {
        EndpointKey::new(*secret.public().as_bytes())
    }

    fn response(secret: &SecretKey, challenge: &EndpointProofChallenge) -> EndpointProofResponse {
        EndpointProofResponse {
            challenge_id: challenge.challenge_id,
            signature: EndpointSignature::from_bytes(
                secret.sign(&challenge.signing_message()).to_bytes(),
            ),
        }
    }

    #[test]
    fn proves_possession_of_the_iroh_endpoint_key_once() {
        let secret = SecretKey::generate();
        let now = OffsetDateTime::now_utc();
        let mut session = EndpointProofSession::new(DeploymentId::new());
        let challenge = session
            .issue(
                EndpointProofPrincipal::User {
                    user_id: UserId::new(),
                },
                TenantId::new(),
                endpoint(&secret),
                EndpointProofPurpose::RegisterUserEndpoint,
                now,
                Duration::from_secs(30),
            )
            .unwrap();
        let verified = session.verify(response(&secret, &challenge), now).unwrap();
        assert_eq!(verified.endpoint_key(), endpoint(&secret));
        assert_eq!(
            session.verify(response(&secret, &challenge), now),
            Err(EndpointProofError::NoPendingChallenge)
        );
    }

    #[test]
    fn rejects_another_key_and_consumes_the_challenge() {
        let claimed = SecretKey::generate();
        let attacker = SecretKey::generate();
        let now = OffsetDateTime::now_utc();
        let mut session = EndpointProofSession::new(DeploymentId::new());
        let challenge = session
            .issue(
                EndpointProofPrincipal::User {
                    user_id: UserId::new(),
                },
                TenantId::new(),
                endpoint(&claimed),
                EndpointProofPurpose::RegisterDevice,
                now,
                Duration::from_secs(30),
            )
            .unwrap();
        assert_eq!(
            session.verify(response(&attacker, &challenge), now),
            Err(EndpointProofError::InvalidSignature)
        );
        assert_eq!(
            session.verify(response(&claimed, &challenge), now),
            Err(EndpointProofError::NoPendingChallenge)
        );
    }

    #[test]
    fn rejects_an_expired_challenge() {
        let secret = SecretKey::generate();
        let now = OffsetDateTime::now_utc();
        let mut session = EndpointProofSession::new(DeploymentId::new());
        let challenge = session
            .issue(
                EndpointProofPrincipal::Device {
                    device_id: pab_protocol::DeviceId::new(),
                },
                TenantId::new(),
                endpoint(&secret),
                EndpointProofPurpose::AuthenticateRegisteredEndpoint,
                now,
                Duration::from_millis(1),
            )
            .unwrap();
        assert_eq!(
            session.verify(
                response(&secret, &challenge),
                now + time::Duration::milliseconds(1)
            ),
            Err(EndpointProofError::Contract(
                EndpointProofContractError::Expired
            ))
        );
    }
}
