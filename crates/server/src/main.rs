use std::{env, fs, net::SocketAddr, path::PathBuf, process::ExitCode, time::Duration};

use pab_protocol::RelayLimitDefaults;
use pab_server::{
    ControlApiConfig, ControlApiState, ControlPlane, PasswordPolicy, PostgresStore,
    RelayControlAuth, serve_tls,
};

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(error) = pab_logging::init("server", std::path::Path::new(".")) {
        eprintln!("pab-server: {error}");
        return ExitCode::FAILURE;
    }
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "server failed");
            eprintln!("pab-server: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let command = env::args().nth(1).unwrap_or_else(|| "check".to_owned());
    let arguments = env::args().skip(2).collect::<Vec<_>>();
    let database_url = env::var("PAB_DATABASE_URL")
        .map_err(|_| "PAB_DATABASE_URL must be set; the value is never printed")?;
    let store = PostgresStore::connect(&database_url, 5).await?;

    match command.as_str() {
        "web-admin" => {
            let [username] = arguments.as_slice() else {
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
            let control_plane = ControlPlane::new(store, PasswordPolicy::default())?;
            control_plane
                .initialize_settings(RelayLimitDefaults {
                    user_mbps: 5,
                    guest_mbps: 1,
                })
                .await?;
            println!("PostgreSQL initialized");
        }
        "account-id" => {
            let [username] = arguments.as_slice() else {
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
            let control_plane = ControlPlane::new(store, PasswordPolicy::default())?;
            let address = env::var("PAB_LISTEN_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:8443".to_owned())
                .parse::<SocketAddr>()?;
            let certificate_path = required_path("PAB_TLS_CERT")?;
            let private_key_path = required_path("PAB_TLS_KEY")?;
            let registration_enabled = env::var("PAB_REGISTRATION_ENABLED")
                .map(|value| value.eq_ignore_ascii_case("true") || value == "1")
                .unwrap_or(true);
            let relay_control_secret = required_secret("PAB_RELAY_CONTROL_SECRET")?;
            let relay_auth = RelayControlAuth::new(&relay_control_secret)?;
            let presence_control = control_plane.clone();
            let github = pab_server::web::GithubConfig::from_env()?;
            let state = ControlApiState::new(
                control_plane,
                ControlApiConfig {
                    registration_enabled,
                    ..ControlApiConfig::default()
                },
                relay_auth,
            ).with_github(github);
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
            let result = serve_tls(address, certificate_path, private_key_path, state).await;
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

fn required_path(name: &'static str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{name} must be set").into())
}

fn required_secret(name: &'static str) -> Result<String, Box<dyn std::error::Error>> {
    if let Ok(value) = env::var(name) {
        return Ok(value);
    }
    let file_name = format!("{name}_FILE");
    let path = env::var_os(&file_name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{name} or {file_name} must be set; the value is never printed"))?;
    let value = fs::read_to_string(path)?;
    let value = value.trim_end_matches(['\r', '\n']).to_owned();
    if value.is_empty() {
        return Err(format!("{file_name} must not be empty").into());
    }
    Ok(value)
}
