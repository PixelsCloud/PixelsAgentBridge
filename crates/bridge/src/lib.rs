#![forbid(unsafe_code)]

mod config;
mod connection;
mod runtime;

pub use config::{BridgeConfig, BridgeConfigError};
pub use connection::{
    AuthenticatedDeviceConnection, BridgeClient, BridgeConnector, BridgeError, TaskSubscription,
};
pub use runtime::{
    BridgeRuntime, BridgeRuntimeConfig, DeviceConnectionPhase, DevicePasswordProvider,
    FileDevicePasswordProvider, LocalTaskRecord, RuntimeCredentialError, RuntimeError,
    RuntimeEvent, RuntimeEventKind,
};
