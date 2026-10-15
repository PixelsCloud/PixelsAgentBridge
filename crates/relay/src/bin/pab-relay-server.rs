use pab_relay::{RelayFileConfig, run_relay_service_until};
use std::process::ExitCode;

#[tokio::main]
async fn console_main() -> ExitCode {
    match run(None).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "relay failed");
            eprintln!("pab-relay: {error}");
            ExitCode::FAILURE
        }
    }
}
fn main() -> ExitCode {
    match pab_service_host::arguments(std::env::args_os().skip(1), "PixelsAgentBridgeRelay") {
        Err(error) => {
            eprintln!("pab-relay: {error}");
            ExitCode::FAILURE
        }
        Ok((Some(name), _)) => {
            #[cfg(windows)]
            {
                match pab_service_host::run(name, service_entry) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("pab-relay: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
            #[cfg(not(windows))]
            {
                let _ = name;
                eprintln!("--service is Windows only");
                ExitCode::FAILURE
            }
        }
        Ok((None, _)) => console_main(),
    }
}
#[cfg(windows)]
fn service_entry(stop: tokio::sync::watch::Receiver<bool>) -> pab_service_host::ServiceFuture {
    Box::pin(async move {
        run(Some(stop)).await.map_err(|e| {
            tracing::error!(error=%e,"Relay service failed");
            e.to_string()
        })
    })
}
async fn run(
    stop: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (_, raw) =
        pab_service_host::arguments(std::env::args_os().skip(1), "PixelsAgentBridgeRelay")?;
    let (path, rest) = pab_service_config::arguments("pab-relay-server.toml", raw)?;
    let check = rest == ["check"];
    if !rest.is_empty() && !check {
        return Err("usage: pab-relay-server [--config path]".into());
    }
    let config = RelayFileConfig::load(&path)?;
    if check {
        pab_relay::check_relay_config(&config.into_service())?;
        println!("Relay configuration and TLS files are valid");
        return Ok(());
    }
    pab_logging::init_configured("relay", &config.log.directory, &config.log.level)?;
    run_relay_service_until(
        config.into_service(),
        pab_service_host::stop_requested(stop),
    )
    .await
}
