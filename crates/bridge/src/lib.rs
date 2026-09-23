#![forbid(unsafe_code)]

mod config;
mod connection;
mod runtime;

pub use config::{BridgeConfig, BridgeConfigError, BridgeIdentity, GuestConfigError};
pub use connection::{
    AuthenticatedDeviceConnection, BridgeClient, BridgeConnector, BridgeError, TaskSubscription,
};
pub use runtime::{
    BridgeRuntime, BridgeRuntimeConfig, DeviceConnectionPhase, DevicePasswordProvider,
    DirectoryDevicePasswordProvider, FileDevicePasswordProvider, LocalTaskRecord,
    MemoryDevicePasswordProvider, RuntimeCredentialError, RuntimeError, RuntimeEvent,
    RuntimeEventKind,
};
