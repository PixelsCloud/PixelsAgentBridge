use std::{env, fs, net::SocketAddr, path::PathBuf, process::ExitCode};

use pab_protocol::{DeploymentId, RelayLimitDefaults};
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
    let database_url = env::var("PAB_DATABASE_URL")
        .map_err(|_| "PAB_DATABASE_URL must be set; the value is never printed")?;
    let store = PostgresStore::connect(&database_url, 5).await?;

    match command.as_str() {
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
            let requested_id = match env::var("PAB_DEPLOYMENT_ID") {
                Ok(value) => value.parse::<DeploymentId>()?,
                Err(env::VarError::NotPresent) => DeploymentId::default(),
                Err(error) => return Err(error.into()),
            };
            let deployment_id = control_plane
                .initialize_deployment(
                    requested_id,
                    RelayLimitDefaults {
                        team_mbps: 20,
                        member_mbps: 4,
                        personal_mbps: 5,
                    },
                )
                .await?;
            println!("PostgreSQL initialized for deployment {deployment_id}");
        }
        "serve" => {
            store.migrate().await?;
            let deployment_id = store.deployment_id().await?;
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
            let state = ControlApiState::new(
                control_plane,
                deployment_id,
                ControlApiConfig {
                    registration_enabled,
                    ..ControlApiConfig::default()
                },
                relay_auth,
            );
            println!("TLS control service listening on {address}");
            tracing::info!(%address, "TLS control service listening");
            serve_tls(address, certificate_path, private_key_path, state).await?;
        }
        _ => return Err("usage: pab-server [check|migrate|init|serve]".into()),
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
