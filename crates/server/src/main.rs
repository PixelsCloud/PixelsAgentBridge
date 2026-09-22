use std::{env, process::ExitCode};

use pab_protocol::{DeploymentId, RelayLimitDefaults};
use pab_server::{ControlPlane, PasswordPolicy, PostgresStore};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
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
            let deployment_id = control_plane
                .initialize_deployment(
                    DeploymentId::new(),
                    RelayLimitDefaults {
                        team_mbps: 20,
                        member_mbps: 4,
                        personal_mbps: 5,
                    },
                )
                .await?;
            println!("PostgreSQL initialized for deployment {deployment_id}");
        }
        _ => return Err("usage: pab-server [check|migrate|init]".into()),
    }
    Ok(())
}
