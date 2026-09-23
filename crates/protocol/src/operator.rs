use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{EndpointKey, UserId};

/// The authenticated actor of a remote task. The account representation stays
/// compatible with task snapshots written before guest access was introduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OperatorRef {
    Account(UserId),
    Guest { guest_endpoint_key: EndpointKey },
}

impl OperatorRef {
    pub const fn guest(endpoint_key: EndpointKey) -> Self {
        Self::Guest {
            guest_endpoint_key: endpoint_key,
        }
    }

    pub fn storage_key(self) -> String {
        match self {
            Self::Account(user_id) => user_id.to_string(),
            Self::Guest { guest_endpoint_key } => {
                let mut key = String::from("guest:");
                for byte in guest_endpoint_key.as_bytes() {
                    use fmt::Write;
                    write!(&mut key, "{byte:02x}").expect("writing to String");
                }
                key
            }
        }
    }
}

impl From<UserId> for OperatorRef {
    fn from(value: UserId) -> Self {
        Self::Account(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_json_stays_compatible_with_existing_task_snapshots() {
        let account = UserId::from_u128(7);
        let encoded = serde_json::to_string(&OperatorRef::Account(account)).unwrap();
        assert_eq!(encoded, serde_json::to_string(&account).unwrap());
        assert_eq!(
            serde_json::from_str::<OperatorRef>(&encoded).unwrap(),
            OperatorRef::Account(account)
        );
        assert_eq!(
            OperatorRef::Account(account).storage_key(),
            account.to_string()
        );
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
