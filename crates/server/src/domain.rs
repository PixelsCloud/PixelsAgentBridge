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
