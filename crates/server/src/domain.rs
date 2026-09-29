use pab_protocol::{DeviceId, EndpointKey, EndpointProofPrincipal, TenantId, UserId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: UserId,
    pub username: String,
    pub personal_tenant_id: TenantId,
}

#[derive(Debug, Clone)]
pub(crate) struct AccountCredential {
    pub account: Account,
    pub password_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamRole {
    Owner,
    Admin,
    Member,
}

impl TeamRole {
    pub(crate) const fn as_db(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }

    pub(crate) fn from_db(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Team {
    pub tenant_id: TenantId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: DeviceId,
    pub code: pab_protocol::DeviceCode,
    pub tenant_id: TenantId,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisteredEndpoint {
    pub endpoint_key: EndpointKey,
    pub tenant_id: TenantId,
    pub principal: EndpointProofPrincipal,
}
