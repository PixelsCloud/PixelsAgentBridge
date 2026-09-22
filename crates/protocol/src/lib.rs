#![forbid(unsafe_code)]

mod control;
mod endpoint_proof;
mod ids;
mod platform;
mod relay_control;
mod relay_policy;
mod task;

pub use control::{
    ControlClientMessage, ControlErrorCode, ControlServerMessage, EndpointRegistration,
    EndpointRegistrationResult,
};
pub use endpoint_proof::{
    ENDPOINT_PROOF_SCHEMA_VERSION, EndpointKey, EndpointProofChallenge, EndpointProofContractError,
    EndpointProofPurpose, EndpointProofResponse, EndpointSignature,
};
pub use ids::{
    ChallengeId, ConnectionId, DeploymentId, DeviceId, RequestId, TaskId, TenantId, UserId,
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
    LimitConfigError, RELAY_POLICY_SCHEMA_VERSION, RelayEndpointOwner, RelayEndpointPolicy,
    RelayLimitDefaults, RelayPolicyError, RelayPolicySnapshot, TeamRelayLimits, TrafficScope,
    mbps_to_bytes_per_second,
};
pub use task::{
    CapabilityRef, OutputAvailability, OutputChunk, OutputRange, OutputStream, TASK_SCHEMA_VERSION,
    TaskCompletion, TaskError, TaskEvent, TaskEventKind, TaskProgress, TaskRef, TaskSnapshot,
    TaskState, TransferDirection, TransferPhase, TransferProgress,
};
