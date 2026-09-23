#![forbid(unsafe_code)]

mod config;
mod connection;
mod runtime;

pub use config::{BridgeConfig, BridgeConfigError, BridgeIdentity, GuestConfigError};
pub use connection::{
    AuthenticatedDeviceConnection, BridgeClient, BridgeConnector, BridgeError, TaskSubscription,
};
pub use runtime::{
    BridgeLocalStore, BridgeRuntime, BridgeRuntimeConfig, DeviceConnectionPhase,
    DevicePasswordProvider, DirectoryDevicePasswordProvider, FileDevicePasswordProvider,
    LocalTaskRecord, MemoryDevicePasswordProvider, RememberedDevice, RuntimeCredentialError,
    RuntimeError, RuntimeEvent, RuntimeEventKind,
};
