#![forbid(unsafe_code)]

mod control;
mod supervisor;
mod tls;

pub use control::{AuthenticatedControlConnection, EndpointControlConfig, EndpointControlError};
pub use supervisor::{
    ConnectionFailure, ConnectionFailureKind, ControlConnectionPhase, ControlConnectionStatus,
    EndpointControlSupervisor, EndpointControlSupervisorHandle, ReconnectPolicy,
    ReconnectPolicyError,
};
pub use tls::{TlsConnectorError, tls_connector};
