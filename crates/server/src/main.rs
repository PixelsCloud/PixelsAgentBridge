use std::{env, fs, net::SocketAddr, path::PathBuf, process::ExitCode, time::Duration};

use pab_protocol::{DeploymentId, RelayLimitDefaults, TenantId, UserId};
use pab_server::{
    ControlApiConfig, ControlApiState, ControlPlane, PasswordPolicy, PostgresStore,
    RelayControlAuth, TeamRole, serve_tls,
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
        "account-id" => {
            let [username] = arguments.as_slice() else {
                return Err("usage: pab-server account-id <username>".into());
            };
            let control = ControlPlane::new(store, PasswordPolicy::default())?;
            println!("{}", control.account_id_by_username(username).await?);
        }
        "team-create" => {
            let [owner_id, name] = arguments.as_slice() else {
                return Err("usage: pab-server team-create <owner-account-id> <team-name>".into());
            };
            store.migrate().await?;
            let owner_id = owner_id.parse::<UserId>()?;
            let operator = required_admin_actor()?;
            let control = ControlPlane::new(store, PasswordPolicy::default())?;
            let team = control.admin_create_team(owner_id, name, &operator).await?;
            println!("Team {} created for account {}", team.tenant_id, owner_id);
        }
        "team-add-member" => {
            let [team_id, user_id, role] = arguments.as_slice() else {
                return Err(
                    "usage: pab-server team-add-member <team-id> <account-id> <admin|member>"
                        .into(),
                );
            };
            store.migrate().await?;
            let team_id = team_id.parse::<TenantId>()?;
            let user_id = user_id.parse::<UserId>()?;
            let role = match role.as_str() {
                "admin" => TeamRole::Admin,
                "member" => TeamRole::Member,
                _ => return Err("role must be admin or member".into()),
            };
            let operator = required_admin_actor()?;
            let control = ControlPlane::new(store, PasswordPolicy::default())?;
            let added = control
                .admin_add_team_member(team_id, user_id, role, &operator)
                .await?;
            println!(
                "Team {team_id} account {user_id}: {}",
                if added {
                    "member added"
                } else {
                    "already a member"
                }
            );
        }
        "team-remove-member" => {
            let [team_id, user_id] = arguments.as_slice() else {
                return Err(
                    "usage: pab-server team-remove-member <team-id> <account-id>".into(),
                );
            };
            store.migrate().await?;
            let team_id = team_id.parse::<TenantId>()?;
            let user_id = user_id.parse::<UserId>()?;
            let operator = required_admin_actor()?;
            let control = ControlPlane::new(store, PasswordPolicy::default())?;
            let removed = control
                .admin_remove_team_member(team_id, user_id, &operator)
                .await?;
            println!(
                "Team {team_id} account {user_id}: {}",
                if removed { "member removed" } else { "not an active member" }
            );
        }
        "team-set-limits" => {
            let [team_id, total_mbps, member_mbps] = arguments.as_slice() else {
                return Err(
                    "usage: pab-server team-set-limits <team-id> <total-mbps> <member-mbps>"
                        .into(),
                );
            };
            store.migrate().await?;
            let team_id = team_id.parse::<TenantId>()?;
            let total_mbps = total_mbps.parse::<u32>()?;
            let member_mbps = member_mbps.parse::<u32>()?;
            let operator = required_admin_actor()?;
            let control = ControlPlane::new(store, PasswordPolicy::default())?;
            let changed = control
                .admin_set_team_limits(team_id, total_mbps, member_mbps, &operator)
                .await?;
            println!(
                "Team {team_id} speeds: {} (total {total_mbps} Mbps, member {member_mbps} Mbps)",
                if changed { "updated" } else { "unchanged" }
            );
        }
        "account-set-default-team" => {
            let [user_id, team_id] = arguments.as_slice() else {
                return Err(
                    "usage: pab-server account-set-default-team <account-id> <team-id|personal>"
                        .into(),
                );
            };
            store.migrate().await?;
            let user_id = user_id.parse::<UserId>()?;
            let team_id = if team_id == "personal" {
                None
            } else {
                Some(team_id.parse::<TenantId>()?)
            };
            let operator = required_admin_actor()?;
            let control = ControlPlane::new(store, PasswordPolicy::default())?;
            let changed = control
                .admin_set_default_traffic_team(user_id, team_id, &operator)
                .await?;
            println!(
                "Account {user_id} default traffic Team: {} ({})",
                team_id.map_or_else(|| "personal".to_owned(), |id| id.to_string()),
                if changed { "updated" } else { "unchanged" }
            );
        }
        "serve" => {
            store.migrate().await?;
            let deployment_id = store.deployment_id().await?;
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
        _ => return Err(
            "usage: pab-server [check|migrate|init|serve|web-admin|account-id|team-create|team-add-member|team-remove-member|team-set-limits|account-set-default-team]"
                .into(),
        ),
    }
    Ok(())
}

fn required_admin_actor() -> Result<String, Box<dyn std::error::Error>> {
    let actor = env::var("PAB_ADMIN_ACTOR")
        .map_err(|_| "PAB_ADMIN_ACTOR must name the server administrator")?;
    if actor.trim().is_empty() {
        return Err("PAB_ADMIN_ACTOR must not be empty".into());
    }
    Ok(actor)
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
