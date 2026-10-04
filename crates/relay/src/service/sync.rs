use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_tungstenite::Connector;

use crate::{PolicySync, RelayControlClient, RelayControlClientError, RelayPolicyRuntime};

const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

pub struct PolicySyncSettings {
    pub control_url: String,
    pub control_secret: String,
    pub refresh_interval: Duration,
    pub reconnect_interval: Duration,
}

pub async fn connect_and_sync(
    settings: &PolicySyncSettings,
    connector: Connector,
    runtime: &RelayPolicyRuntime,
) -> Result<RelayControlClient, RelayControlClientError> {
    let mut client =
        RelayControlClient::connect(&settings.control_url, &settings.control_secret, connector)
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
        let mut last_refresh = tokio::time::Instant::now();
        loop {
            let on_demand = tokio::select! {
                _ = tokio::time::sleep(settings.refresh_interval) => false,
                _ = runtime.wait_for_refresh_request() => true,
            };
            // Coalesce packet-triggered refreshes and bound database work even
            // when an admitted client continually sends to an unauthorized peer.
            tokio::time::sleep_until(last_refresh + MIN_REFRESH_INTERVAL).await;
            match client.sync_policy(&runtime).await {
                Ok(PolicySync::Updated { policy_version }) => {
                    tracing::info!(policy_version, on_demand, "relay policy updated");
                }
                Ok(PolicySync::Unchanged { .. }) => {}
                Err(error) => {
                    tracing::warn!(%error, "relay policy refresh failed");
                    client = reconnect(&settings, connector.clone(), &runtime).await;
                }
            }
            last_refresh = tokio::time::Instant::now();
        }
    })
}

async fn reconnect(
    settings: &PolicySyncSettings,
    connector: Connector,
    runtime: &RelayPolicyRuntime,
) -> RelayControlClient {
    loop {
        tokio::time::sleep(settings.reconnect_interval).await;
        match connect_and_sync(settings, connector.clone(), runtime).await {
            Ok(client) => {
                tracing::info!("relay policy control connection restored");
                return client;
            }
            Err(error) => {
                tracing::warn!(%error, "relay policy reconnect failed");
            }
        }
    }
}
