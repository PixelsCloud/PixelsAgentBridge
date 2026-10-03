#![forbid(unsafe_code)]

mod account_scope;
mod connection_io;
mod connection_state;
mod control;
mod device_network_resolver;
mod enrollment;
mod identity;
mod open_registration;
mod peer_authorizer;
mod storage_paths;
mod supervisor;
mod tls;

pub use account_scope::{
    AccountScopeError, AccountScopeRegistration, login_traffic_scopes,
    register_account_traffic_scope,
};
pub use connection_state::{
    ConnectionFailure, ConnectionFailureKind, ControlConnectionPhase, ControlConnectionStatus,
    DeviceHelloConfigError, DeviceNetworkConfigError, ReconnectPolicy, ReconnectPolicyError,
};
pub use control::{AuthenticatedControlConnection, EndpointControlConfig, EndpointControlError};
pub use device_network_resolver::{DeviceNetworkResolutionError, DeviceNetworkResolver};
pub use enrollment::{EndpointEnrollment, EnrollmentError, enroll_account_with_device};
pub use identity::{
    EndpointSecretError, load_or_create_endpoint_secret, read_endpoint_secret,
    restrict_private_file,
};
pub use open_registration::{OpenRegistrationError, OpenRegistrationKind, register_open_endpoint};
pub use peer_authorizer::{DevicePeerAuthorizer, PeerAuthorizationError};
pub use storage_paths::{
    DataPathError, DataPaths, DataScope, ensure_data_dir, ensure_data_parent, persistent_data_dir,
};
pub use supervisor::{EndpointControlSupervisor, EndpointControlSupervisorHandle};
pub use tls::{TlsConnectorError, tls_connector};
