use serde::{Deserialize, Serialize};

use crate::{DeviceRef, EndpointKey, UserId};

pub const DEVICE_SESSION_AUTH_SCHEMA_VERSION: u16 = 1;
pub const MAX_DEVICE_PASSWORD_BYTES: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizedDevicePeer {
    pub device_ref: DeviceRef,
    pub peer_endpoint_key: EndpointKey,
    pub peer_user_id: UserId,
    pub authorized_at_unix_ms: i64,
}

#[derive(Serialize, Deserialize)]
pub struct DeviceSessionAuthenticate {
    pub schema_version: u16,
    pub device_ref: DeviceRef,
    pub device_password: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DeviceSessionAuthenticationResult {
    Accepted {
        device_ref: DeviceRef,
        peer_user_id: UserId,
        password_version: u64,
        authenticated_at_unix_ms: i64,
    },
    Rejected,
}
