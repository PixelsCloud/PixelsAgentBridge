#![forbid(unsafe_code)]

mod active_endpoints;
pub mod auth;
pub mod control;
pub mod domain;
pub mod endpoint_proof;
pub mod postgres;
pub mod service;

pub use auth::PasswordPolicy;
pub use control::{
    ControlApiConfig, ControlApiState, ControlSession, RelayControlAuth, RelayControlAuthError,
    control_router, serve_tls,
};
pub use domain::{Account, Device, RegisteredEndpoint, Team, TeamInvitation, TeamRole};
pub use endpoint_proof::{EndpointProofError, EndpointProofSession, VerifiedEndpointProof};
pub use postgres::{PostgresStore, StoreError};
pub use service::{ControlPlane, ServiceError};
