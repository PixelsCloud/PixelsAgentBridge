#![forbid(unsafe_code)]

mod control;
mod endpoint_proof;
mod ids;
mod relay_policy;

pub use control::{
    ControlClientMessage, ControlErrorCode, ControlServerMessage, EndpointRegistration,
    EndpointRegistrationResult,
};
pub use endpoint_proof::{
    ENDPOINT_PROOF_SCHEMA_VERSION, EndpointKey, EndpointProofChallenge, EndpointProofContractError,
    EndpointProofPurpose, EndpointProofResponse, EndpointSignature,
};
pub use ids::{ChallengeId, ConnectionId, DeploymentId, DeviceId, RequestId, TenantId, UserId};
pub use relay_policy::{
    LimitConfigError, RELAY_POLICY_SCHEMA_VERSION, RelayEndpointOwner, RelayEndpointPolicy,
    RelayLimitDefaults, RelayPolicyError, RelayPolicySnapshot, TeamRelayLimits, TrafficScope,
    mbps_to_bytes_per_second,
};
