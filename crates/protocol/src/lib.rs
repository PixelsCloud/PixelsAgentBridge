#![forbid(unsafe_code)]
mod app_management;
pub use app_management::*;
mod container;
pub use container::*;
mod desktop_query;
pub use desktop_query::*;
mod desktop_batch;
pub use desktop_batch::*;
mod monitor_input;
pub use monitor_input::*;
mod ui_automation;
pub use ui_automation::*;

mod control;
mod device;
mod device_code;
mod device_network;
mod directory;
mod endpoint_proof;
mod execution_identity;
mod filesystem;
mod ids;
mod operator;
mod peer;
mod platform;
pub use execution_identity::*;
mod file_transfer;
mod relay_control;
mod relay_policy;
mod screenshot;
mod task;
mod task_session;
mod terminal;
pub use file_transfer::*;
pub use terminal::{TerminalStartup, TerminalStartupMode};
mod window;

pub use control::{
    ControlClientMessage, ControlErrorCode, ControlServerMessage, DeviceClaimEntry,
    EndpointAuthenticationResult, EndpointRegistration, EndpointRegistrationResult,
    TrafficScopeOptions,
};
pub use device::{
    DEVICE_SESSION_SCHEMA_VERSION, DeviceDirectoryEntry, DeviceHello, DeviceHelloResult,
    DevicePresence,
};
pub use device_code::{DeviceCode, DeviceCodeError};
pub use device_network::{
    DEVICE_NETWORK_SCHEMA_VERSION, DeviceNetworkResult, DeviceNetworkSnapshot, DeviceNetworkUpdate,
    MAX_DEVICE_DIRECT_ADDRESSES, MAX_DEVICE_RELAY_URLS,
};
pub use directory::{
    DirectoryEntry, DirectoryEntryKind, DirectoryPage, MAX_DIRECTORY_NAME_BYTES,
    MAX_DIRECTORY_PAGE_ENTRIES, MAX_DIRECTORY_PATH_BYTES,
};
pub use endpoint_proof::{
    ENDPOINT_PROOF_CLOCK_SKEW_MS, ENDPOINT_PROOF_SCHEMA_VERSION, EndpointKey,
    EndpointProofChallenge, EndpointProofContractError, EndpointProofPrincipal,
    EndpointProofPurpose, EndpointProofResponse, EndpointSignature,
};
pub use filesystem::{
    FileHashProgress, FileItemResult, FileMetadata, FileMutationSummary, FileOperationLimits,
    FileSearchMatch, FileSearchMode, FileSearchSummary, FileSystemAction, FileSystemError,
    FileSystemReply, FileSystemRequest, MAX_TEXT_EDITS, MAX_TEXT_FILE_BYTES,
    MAX_TEXT_PAYLOAD_BYTES, MAX_TEXT_READ_BYTES, TextEdit, TextEncoding, TextReadPosition,
    TextReadRange, cancellable_filesystem_kind, valid_file_hash,
};
pub use ids::{
    ChallengeId, ClaimId, ConnectionId, DeviceId, EndpointInstanceId, ExecutionContextRef,
    RequestId, TaskId, TenantId, UserId,
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
    LimitConfigError, RELAY_POLICY_SCHEMA_VERSION, RelayConnectionIntent, RelayEndpointOwner,
    RelayEndpointPolicy, RelayLimitDefaults, RelayPolicyError, RelayPolicySnapshot, TrafficScope,
    UserRelayLimit, mbps_to_bytes_per_second,
};
pub use screenshot::*;
pub use task::{
    CapabilityRef, OutputAvailability, OutputChunk, OutputRange, OutputStream, TASK_SCHEMA_VERSION,
    TaskCompletion, TaskError, TaskEvent, TaskEventKind, TaskProgress, TaskRef, TaskSnapshot,
    TaskState, TransferDirection, TransferPhase, TransferProgress,
};
pub use task_session::{
    CommandOptions, CommandTaskSpec, DEVICE_TASK_SCHEMA_VERSION, DesktopInputEvent,
    DesktopMouseButton, DeviceTaskErrorCode, DeviceTaskRequest, DeviceTaskResponse,
    MAX_COMMAND_ARGUMENT_BYTES, MAX_COMMAND_ARGUMENTS, MAX_COMMAND_PROGRAM_BYTES,
    MAX_OUTPUT_READ_BYTES, MAX_TERMINAL_INPUT_BYTES, MAX_TERMINAL_READ_BYTES, TransferSnapshot,
};
pub use window::{MAX_WINDOW_ENTRIES, MAX_WINDOW_TITLE_BYTES, WindowEntry, WindowList};

mod system_query;
pub use system_query::*;
mod git;
pub use git::*;

pub use filesystem::{FileSearchOptions, LogReadState, PatchPreview, SearchContextLine};
mod user_context;
pub use user_context::{
    EndpointUserContext, EndpointUserContextReceipt, EndpointUserContextUpdate,
    RelayUserContextReceipt, UserAttribution,
};
