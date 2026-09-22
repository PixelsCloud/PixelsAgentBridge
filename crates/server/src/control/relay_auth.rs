use subtle::ConstantTimeEq;

const MIN_RELAY_SECRET_BYTES: usize = 32;
const AUTHORIZATION_PREFIX: &str = "Bearer ";

#[derive(Clone)]
pub struct RelayControlAuth {
    expected_digest: [u8; 32],
}

impl RelayControlAuth {
    pub fn new(secret: &str) -> Result<Self, RelayControlAuthError> {
        if secret.len() < MIN_RELAY_SECRET_BYTES {
            return Err(RelayControlAuthError::SecretTooShort);
        }
        Ok(Self {
            expected_digest: *blake3::hash(secret.as_bytes()).as_bytes(),
        })
    }

    pub(crate) fn authorizes(&self, authorization: Option<&str>) -> bool {
        let Some(secret) = authorization.and_then(|value| value.strip_prefix(AUTHORIZATION_PREFIX))
        else {
            return false;
        };
        let digest = blake3::hash(secret.as_bytes());
        bool::from(self.expected_digest.ct_eq(digest.as_bytes()))
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RelayControlAuthError {
    #[error("Relay control secret must contain at least 32 bytes")]
    SecretTooShort,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_the_configured_bearer_secret() {
        let auth = RelayControlAuth::new("a sufficiently long Relay control secret").unwrap();
        assert!(auth.authorizes(Some("Bearer a sufficiently long Relay control secret")));
        assert!(!auth.authorizes(Some("Bearer another sufficiently long Relay secret")));
        assert!(!auth.authorizes(None));
    }
}
