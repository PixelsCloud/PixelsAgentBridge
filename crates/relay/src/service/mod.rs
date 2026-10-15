mod config;
#[cfg(test)]
mod config_tests;
mod sync;
mod tls;

pub use config::{RelayFileConfig, RelayServiceConfig, RelayServiceConfigError};
pub use runner::{
    RunningRelayService, check_relay_config, run_relay_service, run_relay_service_until,
    start_relay_service,
};

mod runner;
