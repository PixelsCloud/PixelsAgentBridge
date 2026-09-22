#![forbid(unsafe_code)]

mod connection_io;
mod connection_state;
mod control;
mod device_network_resolver;
mod identity;
mod peer_authorizer;
mod supervisor;
mod tls;

pub use connection_state::{
    ConnectionFailure, ConnectionFailureKind, ControlConnectionPhase, ControlConnectionStatus,
    DeviceHelloConfigError, DeviceNetworkConfigError, ReconnectPolicy, ReconnectPolicyError,
};
pub use control::{AuthenticatedControlConnection, EndpointControlConfig, EndpointControlError};
pub use device_network_resolver::{DeviceNetworkResolutionError, DeviceNetworkResolver};
pub use identity::{EndpointSecretError, read_endpoint_secret};
pub use peer_authorizer::{DevicePeerAuthorizer, PeerAuthorizationError};
pub use supervisor::{EndpointControlSupervisor, EndpointControlSupervisorHandle};
pub use tls::{TlsConnectorError, tls_connector};
