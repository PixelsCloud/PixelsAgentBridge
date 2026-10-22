#![forbid(unsafe_code)]

mod active_endpoints;
pub mod audit;
pub mod auth;
pub mod control;
pub mod domain;
pub mod endpoint_proof;
pub mod postgres;
mod relay_budget;
pub mod relay_cluster;
mod relay_probe;
mod relay_diagnostics;
pub mod service;
pub mod web;

pub use auth::PasswordPolicy;
pub use control::{
    ControlApiConfig, ControlApiState, ControlSession, RelayControlAuth, RelayControlAuthError,
    control_router, serve_tls, serve_tls_until,
};
pub use domain::{Account, Device, RegisteredEndpoint};
pub use endpoint_proof::{EndpointProofError, EndpointProofSession, VerifiedEndpointProof};
pub use postgres::{PostgresStore, StoreError};
pub use service::{ControlPlane, ServiceError};

pub mod config;
