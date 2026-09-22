use std::process::ExitCode;

use pab_relay::{RelayServiceConfig, run_relay_service};

#[tokio::main]
async fn main() -> ExitCode {
    match RelayServiceConfig::from_env() {
        Ok(config) => match run_relay_service(config).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("pab-relay: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("pab-relay: {error}");
            ExitCode::FAILURE
        }
    }
}
