#![forbid(unsafe_code)]

mod config;
mod connection;
pub mod desktop_presence;
mod runtime;

pub use config::{BridgeConfig, BridgeConfigError, BridgeIdentity, GuestConfigError};
pub use connection::{
    AuthenticatedDeviceConnection, BridgeClient, BridgeConnector, BridgeError, Screenshot,
    TaskSubscription,
};
pub use pab_transport::ConnectionPath;
pub use runtime::{
    BridgeLocalStore, BridgeRuntime, BridgeRuntimeConfig, DeviceConnectionPhase,
    DevicePasswordProvider, DirectoryDevicePasswordProvider, FileDevicePasswordProvider,
    LocalTaskRecord, MemoryDevicePasswordProvider, OperationRecord, RememberedDevice,
    RuntimeCredentialError, RuntimeError, RuntimeEvent, RuntimeEventKind, RuntimePresenceSource,
    SqliteDevicePasswordProvider, TerminalAuditEvent,
};
