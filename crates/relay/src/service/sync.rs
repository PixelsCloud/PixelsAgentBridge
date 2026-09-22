use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_tungstenite::Connector;

use crate::{RelayControlClient, RelayControlClientError, RelayPolicyRuntime};

pub struct PolicySyncSettings {
    pub control_url: String,
    pub deployment_id: pab_protocol::DeploymentId,
    pub control_secret: String,
    pub refresh_interval: Duration,
    pub reconnect_initial_delay: Duration,
    pub reconnect_max_delay: Duration,
}

pub async fn connect_and_sync(
    settings: &PolicySyncSettings,
    connector: Connector,
    runtime: &RelayPolicyRuntime,
) -> Result<RelayControlClient, RelayControlClientError> {
    let mut client = RelayControlClient::connect(
        &settings.control_url,
        settings.deployment_id,
        &settings.control_secret,
        connector,
    )
    .await?;
    client.sync_policy(runtime).await?;
    Ok(client)
}

pub fn spawn_refresh_loop(
    mut client: RelayControlClient,
    settings: PolicySyncSettings,
    connector: Connector,
    runtime: RelayPolicyRuntime,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(settings.refresh_interval).await;
            if let Err(error) = client.sync_policy(&runtime).await {
                eprintln!("pab-relay: policy refresh failed: {error}");
                client = reconnect(&settings, connector.clone(), &runtime).await;
            }
        }
    })
}

async fn reconnect(
    settings: &PolicySyncSettings,
    connector: Connector,
    runtime: &RelayPolicyRuntime,
) -> RelayControlClient {
    let mut delay = settings.reconnect_initial_delay;
    loop {
        tokio::time::sleep(delay).await;
        match connect_and_sync(settings, connector.clone(), runtime).await {
            Ok(client) => {
                eprintln!("pab-relay: policy control connection restored");
                return client;
            }
            Err(error) => {
                eprintln!("pab-relay: policy reconnect failed: {error}");
                delay = delay.saturating_mul(2).min(settings.reconnect_max_delay);
            }
        }
    }
}
