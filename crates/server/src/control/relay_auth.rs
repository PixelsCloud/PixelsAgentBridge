use subtle::ConstantTimeEq;

const MIN_RELAY_SECRET_BYTES: usize = 32;
const AUTHORIZATION_PREFIX: &str = "Bearer ";

#[derive(Clone)]
pub struct RelayControlAuth {
    expected_digest: [u8; 32],
    nodes: std::collections::BTreeMap<String, [u8; 32]>,
    legacy_enabled: bool,
}

impl RelayControlAuth {
    pub fn new(secret: &str) -> Result<Self, RelayControlAuthError> {
        if secret.len() < MIN_RELAY_SECRET_BYTES {
            return Err(RelayControlAuthError::SecretTooShort);
        }
        Ok(Self {
            expected_digest: *blake3::hash(secret.as_bytes()).as_bytes(),
            nodes: Default::default(),
            legacy_enabled: true,
        })
    }

    #[cfg(test)]
    pub(crate) fn authorizes(&self, authorization: Option<&str>) -> bool {
        matches!(self.identity(authorization), Some(None))
    }

    pub fn with_cluster(
        mut self,
        config: Option<&crate::relay_cluster::RelayClusterConfig>,
    ) -> Self {
        if let Some(config) = config {
            self.nodes = config
                .nodes
                .iter()
                .map(|n| {
                    (
                        n.node_id.clone(),
                        *blake3::hash(n.control_secret.as_bytes()).as_bytes(),
                    )
                })
                .collect();
            self.legacy_enabled = false;
        }
        self
    }

    pub(crate) fn cluster_identity(
        authorization: Option<&str>,
        config: &crate::relay_cluster::RelayClusterConfig,
    ) -> Option<String> {
        let supplied = authorization.and_then(|s| s.strip_prefix(AUTHORIZATION_PREFIX))?;
        let digest = blake3::hash(supplied.as_bytes());
        let mut matched = None;
        for node in &config.nodes {
            if bool::from(
                blake3::hash(node.control_secret.as_bytes())
                    .as_bytes()
                    .ct_eq(digest.as_bytes()),
            ) {
                matched = Some(node.node_id.clone());
            }
        }
        matched
    }

    /// Some(None) is the legacy identity; Some(Some(id)) is node-bound.
    pub(crate) fn identity(&self, authorization: Option<&str>) -> Option<Option<String>> {
        let Some(secret) = authorization.and_then(|value| value.strip_prefix(AUTHORIZATION_PREFIX))
        else {
            return None;
        };
        let digest = blake3::hash(secret.as_bytes());
        let mut identity = None;
        for (node, expected) in &self.nodes {
            if bool::from(expected.ct_eq(digest.as_bytes())) {
                identity = Some(Some(node.clone()));
            }
        }
        if self.legacy_enabled && bool::from(self.expected_digest.ct_eq(digest.as_bytes())) {
            identity = Some(None);
        }
        identity
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
