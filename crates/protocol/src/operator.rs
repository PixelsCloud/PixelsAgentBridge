use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{EndpointKey, UserId};

/// The authenticated actor of a remote task, including the verified endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OperatorRef {
    Account {
        user_id: UserId,
        endpoint_key: EndpointKey,
    },
    Guest {
        guest_endpoint_key: EndpointKey,
    },
}

impl OperatorRef {
    pub const fn account(user_id: UserId, endpoint_key: EndpointKey) -> Self {
        Self::Account {
            user_id,
            endpoint_key,
        }
    }

    pub const fn guest(endpoint_key: EndpointKey) -> Self {
        Self::Guest {
            guest_endpoint_key: endpoint_key,
        }
    }

    pub const fn endpoint_key(self) -> EndpointKey {
        match self {
            Self::Account { endpoint_key, .. } => endpoint_key,
            Self::Guest { guest_endpoint_key } => guest_endpoint_key,
        }
    }

    pub fn storage_key(self) -> String {
        match self {
            Self::Account {
                user_id,
                endpoint_key,
            } => format!("account:{user_id}:{}", hex_key(endpoint_key)),
            Self::Guest { guest_endpoint_key } => {
                format!("guest:{}", hex_key(guest_endpoint_key))
            }
        }
    }
}

fn hex_key(endpoint_key: EndpointKey) -> String {
    let mut key = String::with_capacity(64);
    for byte in endpoint_key.as_bytes() {
        use fmt::Write;
        write!(&mut key, "{byte:02x}").expect("writing to String");
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_identity_includes_verified_endpoint() {
        let account = UserId::from_u128(7);
        let endpoint = EndpointKey::new([0xab; 32]);
        let actor = OperatorRef::account(account, endpoint);
        let encoded = serde_json::to_string(&actor).unwrap();
        assert_eq!(
            serde_json::from_str::<OperatorRef>(&encoded).unwrap(),
            actor
        );
        assert_eq!(
            actor.storage_key(),
            format!("account:{account}:{}", "ab".repeat(32))
        );
        assert_eq!(actor.endpoint_key(), endpoint);
    }

    #[test]
    fn guest_identity_is_distinct_from_accounts() {
        let key = EndpointKey::new([0xab; 32]);
        let guest = OperatorRef::guest(key);
        assert_eq!(guest.storage_key(), format!("guest:{}", "ab".repeat(32)));
        assert_eq!(
            serde_json::from_str::<OperatorRef>(&serde_json::to_string(&guest).unwrap()).unwrap(),
            guest
        );
    }
}
