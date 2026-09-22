use serde::{Deserialize, Serialize};

use crate::{
    AuthorizedDevicePeer, DeviceHello, DeviceHelloResult, DeviceId, DeviceNetworkResult,
    DeviceNetworkSnapshot, DeviceNetworkUpdate, DeviceRef, EndpointKey, EndpointProofChallenge,
    EndpointProofPrincipal, EndpointProofResponse, RequestId, TenantId, UserId,
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
    SetDeviceConnectGrant {
        request_id: RequestId,
        tenant_id: TenantId,
        device_id: DeviceId,
        user_id: UserId,
        allowed: bool,
    },
    AuthorizeDevicePeer {
        request_id: RequestId,
        peer_endpoint_key: EndpointKey,
    },
}

impl ControlClientMessage {
    pub const fn request_id(&self) -> RequestId {
        match self {
            Self::RegisterAccount { request_id, .. }
            | Self::Login { request_id, .. }
            | Self::BeginEndpointRegistration { request_id, .. }
            | Self::CompleteEndpointRegistration { request_id, .. }
            | Self::BeginEndpointAuthentication { request_id, .. }
            | Self::CompleteEndpointAuthentication { request_id, .. }
            | Self::PublishDeviceHello { request_id, .. }
            | Self::PublishDeviceNetwork { request_id, .. }
            | Self::GetDeviceNetwork { request_id, .. }
            | Self::SetDeviceConnectGrant { request_id, .. }
            | Self::AuthorizeDevicePeer { request_id, .. } => *request_id,
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
    DeviceConnectGrantUpdated {
        request_id: RequestId,
        tenant_id: TenantId,
        device_id: DeviceId,
        user_id: UserId,
        allowed: bool,
    },
    DevicePeerAuthorized {
        request_id: RequestId,
        result: AuthorizedDevicePeer,
    },
    Error {
        request_id: Option<RequestId>,
        code: ControlErrorCode,
        message: String,
    },
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
        endpoint_key: EndpointKey,
    },
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
    NotFound,
    Internal,
}
