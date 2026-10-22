use std::{env, process::ExitCode, time::Duration};

use pab_server::{
    ControlApiConfig, ControlApiState, ControlPlane, PasswordPolicy, PostgresStore,
    RelayControlAuth, serve_tls_until,
};

#[tokio::main]
async fn console_main() -> ExitCode {
    match run(None).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "server failed");
            eprintln!("pab-server: {error}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    match pab_service_host::arguments(env::args_os().skip(1), "PixelsAgentBridgeServer") {
        Err(error) => {
            eprintln!("pab-server: {error}");
            ExitCode::FAILURE
        }
        Ok((Some(name), _)) => {
            #[cfg(windows)]
            {
                match pab_service_host::run(name, service_entry) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("pab-server: {e}");
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
            tracing::error!(error=%e,"Server service failed");
            e.to_string()
        })
    })
}
async fn run(
    stop: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (_, raw) = pab_service_host::arguments(env::args_os().skip(1), "PixelsAgentBridgeServer")?;
    let (path, arguments) = pab_service_config::arguments("pab-server.toml", raw)?;
    let (command, arguments) = arguments
        .split_first()
        .map(|(c, a)| (c.as_str(), a))
        .unwrap_or(("check", &[]));
    if !matches!(
        command,
        "check" | "migrate" | "init" | "serve" | "web-admin" | "account-id"
    ) {
        return Err(
            "usage: pab-server [check|migrate|init|serve|web-admin|account-id] [--config path]"
                .into(),
        );
    }
    if !matches!(command, "web-admin" | "account-id") && !arguments.is_empty() {
        return Err("unexpected command arguments".into());
    }
    let config = pab_server::config::ServerConfig::load(&path)?;
    pab_logging::init_configured("server", &config.log.directory, &config.log.level)?;
    let store = PostgresStore::connect(&config.database.url, 5).await?;

    match command {
        "web-admin" => {
            let [username] = arguments else {
                return Err("usage: pab-server web-admin <existing-username>".into());
            };
            store.migrate().await?;
            let control = ControlPlane::new(store, PasswordPolicy::default())?;
            pab_server::web::bootstrap_admin(&control, username).await?;
            println!("Server administrator enabled");
        }
        "check" => {
            sqlx::query("SELECT 1").execute(store.pool()).await?;
            println!("PostgreSQL connection OK");
        }
        "migrate" => {
            store.migrate().await?;
            println!("PostgreSQL schema is current");
        }
        "init" => {
            store.migrate().await?;
            store.initialize_settings().await?;
            println!("PostgreSQL initialized");
        }
        "account-id" => {
            let [username] = arguments else {
                return Err("usage: pab-server account-id <username>".into());
            };
            let control = ControlPlane::new(store, PasswordPolicy::default())?;
            println!("{}", control.account_id_by_username(username).await?);
        }
        "serve" => {
            store.migrate().await?;
            let initialized: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_settings WHERE singleton)")
                    .fetch_one(store.pool())
                    .await?;
            if !initialized {
                return Err("server settings are not initialized; run pab-server init".into());
            }
            let maintenance_store = store.clone();
            let directory = config.relay.resolved_directory(&store).await?;
            let control_plane = ControlPlane::new(store, PasswordPolicy::default())?
                .with_relay_cluster(directory.clone())
                .await?
                .with_relay_config_source(path.clone(), &config.relay.control_secret);
            let address = config.listen;
            let certificate_path = config.tls.cert.clone();
            let private_key_path = config.tls.key.clone();
            let registration_enabled = config.web.registration_enabled;
            let relay_auth = RelayControlAuth::new(&config.relay.control_secret)?
                .with_cluster(directory.as_ref());
            let presence_control = control_plane.clone();
            let github = config.github_config()?;
            let state = ControlApiState::new(
                control_plane,
                ControlApiConfig {
                    registration_enabled,
                    web_origin: Some(config.web.origin.clone()),
                    web_assets: Some(config.web.assets.clone()),
                    ..ControlApiConfig::default()
                },
                relay_auth,
            )
            .with_github(github);
            println!("TLS control service listening on {address}");
            tracing::info!(%address, "TLS control service listening");
            let maintenance = tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(10 * 60));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    interval.tick().await;
                    match maintenance_store.delete_expired_connection_intents().await {
                        Ok(0) => {}
                        Ok(count) => tracing::info!(count, "expired connection intents removed"),
                        Err(error) => tracing::warn!(%error, "connection intent cleanup failed"),
                    }
                }
            });
            let presence = tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(30));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    interval.tick().await;
                    if presence_control.touch_online_devices().await.is_err() {
                        tracing::warn!("could not checkpoint online device timestamps");
                    }
                }
            });
            let result = serve_tls_until(
                address,
                certificate_path,
                private_key_path,
                state,
                pab_service_host::stop_requested(stop),
            )
            .await;
            maintenance.abort();
            presence.abort();
            result?;
        }
        _ => {
            return Err("usage: pab-server [check|migrate|init|serve|web-admin|account-id]".into());
        }
    }
    Ok(())
}
