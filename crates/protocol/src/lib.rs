#![forbid(unsafe_code)]

mod control;
mod device;
mod device_code;
mod device_network;
mod endpoint_proof;
mod ids;
mod operator;
mod peer;
mod platform;
mod relay_control;
mod relay_policy;
mod task;
mod task_session;

pub use control::{
    ControlClientMessage, ControlErrorCode, ControlServerMessage, EndpointAuthenticationResult,
    EndpointRegistration, EndpointRegistrationResult,
};
pub use device::{
    DEVICE_SESSION_SCHEMA_VERSION, DeviceDirectoryEntry, DeviceHello, DeviceHelloResult,
};
pub use device_code::{DeviceCode, DeviceCodeError};
pub use device_network::{
    DEVICE_NETWORK_SCHEMA_VERSION, DeviceNetworkResult, DeviceNetworkSnapshot, DeviceNetworkUpdate,
    MAX_DEVICE_DIRECT_ADDRESSES, MAX_DEVICE_RELAY_URLS,
};
pub use endpoint_proof::{
    ENDPOINT_PROOF_CLOCK_SKEW_MS, ENDPOINT_PROOF_SCHEMA_VERSION, EndpointKey,
    EndpointProofChallenge, EndpointProofContractError, EndpointProofPrincipal,
    EndpointProofPurpose, EndpointProofResponse, EndpointSignature,
};
pub use ids::{
    ChallengeId, ClaimId, ConnectionId, DeploymentId, DeviceId, EndpointInstanceId, RequestId,
    TaskId, TenantId, UserId,
};
pub use operator::OperatorRef;
pub use peer::{
    AuthorizedDevicePeer, DEVICE_SESSION_AUTH_SCHEMA_VERSION, DeviceSessionAuthenticate,
    DeviceSessionAuthenticationResult, MAX_DEVICE_PASSWORD_BYTES,
};
pub use platform::{
    ContextFreshness, CpuArchitecture, DeviceRef, ExecutionContext, ExecutionScope,
    ExpectedEnvironment, InterpreterContext, OsFamily, PathStyle, TargetContext,
    TargetContextSource,
};
pub use relay_control::{
    RelayControlClientMessage, RelayControlErrorCode, RelayControlServerMessage,
};
pub use relay_policy::{
    GuestRelayGrant, LimitConfigError, RELAY_POLICY_SCHEMA_VERSION, RelayEndpointOwner,
    RelayEndpointPolicy, RelayLimitDefaults, RelayPolicyError, RelayPolicySnapshot,
    TeamRelayLimits, TrafficScope, mbps_to_bytes_per_second,
};
pub use task::{
    CapabilityRef, OutputAvailability, OutputChunk, OutputRange, OutputStream, TASK_SCHEMA_VERSION,
    TaskCompletion, TaskError, TaskEvent, TaskEventKind, TaskProgress, TaskRef, TaskSnapshot,
    TaskState, TransferDirection, TransferPhase, TransferProgress,
};
pub use task_session::{
    CommandTaskSpec, DEVICE_TASK_SCHEMA_VERSION, DeviceTaskErrorCode, DeviceTaskRequest,
    DeviceTaskResponse, MAX_COMMAND_ARGUMENT_BYTES, MAX_COMMAND_ARGUMENTS,
    MAX_COMMAND_PROGRAM_BYTES, MAX_OUTPUT_READ_BYTES,
};
