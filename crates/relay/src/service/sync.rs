use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_tungstenite::Connector;

use crate::{PolicySync, RelayControlClient, RelayControlClientError, RelayPolicyRuntime};

const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct PolicySyncSettings {
    pub process_boot_id: uuid::Uuid,
    pub public_url: Option<String>,
    pub metrics: Option<std::sync::Arc<iroh_relay::server::Metrics>>,
    pub boot_id: Option<uuid::Uuid>,
    pub node_id: String,
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
            .await?
            .with_node_id(settings.node_id.clone());
    if let Some(boot) = settings.boot_id {
        let (_, capacity) = client.cluster_hello(&settings.node_id, boot).await?;
        runtime.set_node_capacity(capacity);
    }
    client.sync_policy(runtime).await?;
    // Report the version actually applied, so Server can distinguish offered/applied.
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
        let mut refresh = tokio::time::interval(
            client
                .heartbeat_interval()
                .unwrap_or(settings.refresh_interval),
        );
        refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut budgets = tokio::time::interval(Duration::from_secs(1));
        budgets.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let on_demand = tokio::select! {
                _ = budgets.tick(), if settings.boot_id.is_some()=>{
                    if let Err(error)=client.sync_budgets(settings.boot_id.unwrap(),&runtime).await {
                        tracing::debug!(%error,"Relay budget not available yet");
                        if !matches!(error,RelayControlClientError::Server{..}){
                            client=reconnect(&settings,connector.clone(),&runtime).await;continue;
                        }
                    }
                    if let Err(error)=client.sync_probe(settings.boot_id.unwrap(),&runtime).await{
                        tracing::debug!(%error,"Relay probe synchronization failed");
                        client=reconnect(&settings,connector.clone(),&runtime).await;
                    }
                    continue;
                }
                _ = refresh.tick() => false,
                _ = runtime.wait_for_refresh_request() => true,
                _ = client.wait_for_policy_change() => true,
            };
            // Coalesce packet-triggered refreshes and bound database work even
            // when an admitted client continually sends to an unauthorized peer.
            tokio::time::sleep_until(last_refresh + MIN_REFRESH_INTERVAL).await;
            let telemetry = if settings.boot_id.is_none() {
                settings
                    .metrics
                    .as_ref()
                    .map(|metrics| pab_protocol::RelayNodeTelemetry {
                        boot_id: settings.process_boot_id,
                        public_url: settings.public_url.clone(),
                        connections: metrics
                            .accepts
                            .get()
                            .saturating_sub(metrics.disconnects.get()),
                        received_bytes: metrics.bytes_recv.get(),
                        sent_bytes: metrics.bytes_sent.get(),
                        diagnostics: None,
                    })
            } else {
                None
            };
            match client.sync_policy_with_telemetry(&runtime, telemetry).await {
                Ok(PolicySync::Updated { policy_version }) => {
                    tracing::info!(policy_version, on_demand, "relay policy updated");
                    if let Err(error) = client.sync_policy(&runtime).await {
                        tracing::warn!(%error, "relay policy acknowledgment failed");
                        client = reconnect(&settings, connector.clone(), &runtime).await;
                    }
                }
                Ok(PolicySync::Unchanged { .. }) => {}
                Err(error) => {
                    tracing::warn!(%error, "relay policy refresh failed");
                    client = reconnect(&settings, connector.clone(), &runtime).await;
                }
            }
            last_refresh = tokio::time::Instant::now();
            let desired = client
                .heartbeat_interval()
                .unwrap_or(settings.refresh_interval);
            if refresh.period() != desired {
                refresh = tokio::time::interval(desired);
                refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            }
            if let (Some(boot), Some(metrics)) = (settings.boot_id, settings.metrics.as_ref()) {
                if let Err(error) = client.heartbeat(boot, metrics, &runtime).await {
                    tracing::debug!(%error,"Relay telemetry report failed");
                }
            }
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
