use pab_relay::{RelayFileConfig, run_relay_service};
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "relay failed");
            eprintln!("pab-relay: {error}");
            ExitCode::FAILURE
        }
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (path, rest) =
        pab_service_config::arguments("pab-relay-server.toml", std::env::args_os().skip(1))?;
    if !rest.is_empty() {
        return Err("usage: pab-relay-server [--config path]".into());
    }
    let config = RelayFileConfig::load(&path)?;
    pab_logging::init_configured("relay", &config.log.directory, &config.log.level)?;
    run_relay_service(config.into_service()).await
}
