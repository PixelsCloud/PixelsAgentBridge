use std::process::ExitCode;

use pab_relay::{RelayServiceConfig, run_relay_service};

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(error) = pab_logging::init("relay", std::path::Path::new(".")) {
        eprintln!("pab-relay: {error}");
        return ExitCode::FAILURE;
    }
    match RelayServiceConfig::from_env() {
        Ok(config) => match run_relay_service(config).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(%error, "relay failed");
                eprintln!("pab-relay: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            tracing::error!(%error, "relay configuration failed");
            eprintln!("pab-relay: {error}");
            ExitCode::FAILURE
        }
    }
}
