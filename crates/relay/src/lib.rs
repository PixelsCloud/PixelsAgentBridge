#![forbid(unsafe_code)]

mod control_client;
mod limiter;
mod policy;
mod runtime;
#[cfg(test)]
mod runtime_tests;
mod service;
mod usage;

pub use control_client::{PolicySync, RelayControlClient, RelayControlClientError};
pub use limiter::{Acquire, AggregateLimiter, LimitKey, Rate};
pub use policy::{PolicyStateError, RelayPolicyState};
pub use runtime::{PolicyRuntimeError, RelayPolicyRuntime};
pub use service::{
    RelayServiceConfig, RelayServiceConfigError, RunningRelayService, run_relay_service,
    start_relay_service,
};
