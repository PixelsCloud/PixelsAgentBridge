use serde::{Deserialize, Serialize};

use crate::{
    AuthorizedDevicePeer, ClaimId, DeviceCode, DeviceDirectoryEntry, DeviceHello,
    DeviceHelloResult, DeviceId, DeviceNetworkResult, DeviceNetworkSnapshot, DeviceNetworkUpdate,
    DevicePresence, DeviceRef, EndpointKey, EndpointProofChallenge, EndpointProofPrincipal,
    EndpointProofResponse, RequestId, TenantId, UserId,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlClientMessage {
    RegisterAccount {
        request_id: RequestId,
        username: String,
        password: String,
    },
    Login {
        request_id: RequestId,
        username: String,
        password: String,
    },
    BeginUnclaimedDeviceRegistration {
        request_id: RequestId,
        endpoint_key: EndpointKey,
        name: String,
    },
    BeginGuestEndpointRegistration {
        request_id: RequestId,
        endpoint_key: EndpointKey,
    },
    BeginEndpointRegistration {
        request_id: RequestId,
        tenant_id: TenantId,
        endpoint_key: EndpointKey,
        registration: EndpointRegistration,
    },
    CompleteEndpointRegistration {
        request_id: RequestId,
        proof: EndpointProofResponse,
    },
    BeginEndpointAuthentication {
        request_id: RequestId,
        endpoint_key: EndpointKey,
    },
    CompleteEndpointAuthentication {
        request_id: RequestId,
        proof: EndpointProofResponse,
    },
    PublishDeviceHello {
        request_id: RequestId,
        hello: Box<DeviceHello>,
    },
    PublishDeviceNetwork {
        request_id: RequestId,
        update: Box<DeviceNetworkUpdate>,
    },
    GetDeviceNetwork {
        request_id: RequestId,
        device_ref: DeviceRef,
    },
    ResolveDeviceCode {
        request_id: RequestId,
        device_code: DeviceCode,
    },
    GetDevicePresence {
        request_id: RequestId,
        device_code: DeviceCode,
    },
    ListDevices {
        request_id: RequestId,
    },
    ListTrafficScopes {
        request_id: RequestId,
    },
    AuthorizeDevicePeer {
        request_id: RequestId,
        peer_endpoint_key: EndpointKey,
    },
    // Legacy wire messages: retained for decoding only. The server rejects all claims.
    BeginDeviceClaim {
        request_id: RequestId,
        device_code: DeviceCode,
        owner_tenant_id: TenantId,
    },
    ApproveDeviceClaim {
        request_id: RequestId,
        claim_id: ClaimId,
    },
    ListDeviceClaims {
        request_id: RequestId,
    },
    RejectDeviceClaim {
        request_id: RequestId,
        claim_id: ClaimId,
    },
}

impl ControlClientMessage {
    pub const fn request_id(&self) -> RequestId {
        match self {
            Self::RegisterAccount { request_id, .. }
            | Self::Login { request_id, .. }
            | Self::BeginUnclaimedDeviceRegistration { request_id, .. }
            | Self::BeginGuestEndpointRegistration { request_id, .. }
            | Self::BeginEndpointRegistration { request_id, .. }
            | Self::CompleteEndpointRegistration { request_id, .. }
            | Self::BeginEndpointAuthentication { request_id, .. }
            | Self::CompleteEndpointAuthentication { request_id, .. }
            | Self::PublishDeviceHello { request_id, .. }
            | Self::PublishDeviceNetwork { request_id, .. }
            | Self::GetDeviceNetwork { request_id, .. }
            | Self::ResolveDeviceCode { request_id, .. }
            | Self::GetDevicePresence { request_id, .. }
            | Self::ListDevices { request_id }
            | Self::ListTrafficScopes { request_id }
            | Self::AuthorizeDevicePeer { request_id, .. } => *request_id,
            Self::BeginDeviceClaim { request_id, .. }
            | Self::ListDeviceClaims { request_id }
            | Self::RejectDeviceClaim { request_id, .. }
            | Self::ApproveDeviceClaim { request_id, .. } => *request_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EndpointRegistration {
    User,
    Device { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlServerMessage {
    AccountAuthenticated {
        request_id: RequestId,
        user_id: UserId,
        username: String,
        personal_tenant_id: TenantId,
    },
    EndpointChallenge {
        request_id: RequestId,
        challenge: EndpointProofChallenge,
    },
    EndpointRegistered {
        request_id: RequestId,
        result: EndpointRegistrationResult,
    },
    EndpointAuthenticated {
        request_id: RequestId,
        result: EndpointAuthenticationResult,
    },
    DeviceHelloAccepted {
        request_id: RequestId,
        result: DeviceHelloResult,
    },
    DeviceNetworkAccepted {
        request_id: RequestId,
        result: DeviceNetworkResult,
    },
    DeviceNetworkFound {
        request_id: RequestId,
        snapshot: Box<DeviceNetworkSnapshot>,
    },
    DeviceCodeResolved {
        request_id: RequestId,
        device_ref: DeviceRef,
    },
    DevicePresenceFound {
        request_id: RequestId,
        presence: DevicePresence,
    },
    DeviceList {
        request_id: RequestId,
        devices: Vec<DeviceDirectoryEntry>,
    },
    TrafficScopeList {
        request_id: RequestId,
        options: TrafficScopeOptions,
    },
    DevicePeerAuthorized {
        request_id: RequestId,
        result: AuthorizedDevicePeer,
    },
    DeviceClaimPending {
        request_id: RequestId,
        claim_id: ClaimId,
    },
    DeviceClaimApproved {
        request_id: RequestId,
        claim_id: ClaimId,
        device_id: DeviceId,
        owner_tenant_id: TenantId,
    },
    DeviceClaims {
        request_id: RequestId,
        claims: Vec<DeviceClaimEntry>,
    },
    DeviceClaimRejected {
        request_id: RequestId,
        claim_id: ClaimId,
    },
    Error {
        request_id: Option<RequestId>,
        code: ControlErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceClaimEntry {
    pub claim_id: ClaimId,
    pub username: String,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointAuthenticationResult {
    pub tenant_id: TenantId,
    pub endpoint_key: EndpointKey,
    pub principal: EndpointProofPrincipal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EndpointRegistrationResult {
    User {
        tenant_id: TenantId,
        endpoint_key: EndpointKey,
    },
    Device {
        tenant_id: TenantId,
        device_id: DeviceId,
        device_code: DeviceCode,
        endpoint_key: EndpointKey,
    },
    Guest {
        tenant_id: TenantId,
        endpoint_key: EndpointKey,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficScopeOptions {
    pub personal_tenant_id: TenantId,
    pub default_tenant_id: TenantId,
    pub personal_mbps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlErrorCode {
    InvalidMessage,
    InvalidState,
    InvalidCredentials,
    RegistrationDisabled,
    PermissionDenied,
    Conflict,
    RateLimited,
    NotFound,
    Internal,
}
