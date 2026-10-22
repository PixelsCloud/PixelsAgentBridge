use std::{error::Error, net::SocketAddr, sync::Arc};

use iroh_relay::server::{CertConfig, QuicConfig, RelayConfig, Server, ServerConfig, TlsConfig};

use super::{
    RelayServiceConfig,
    sync::{PolicySyncSettings, connect_and_sync, spawn_refresh_loop},
    tls::{control_connector, load_server_config},
};
use crate::{RelayPolicyRuntime, RelayPolicyState};

type ServiceResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub fn check_relay_config(config: &RelayServiceConfig) -> ServiceResult<()> {
    config.validate()?;
    load_server_config(&config.tls_cert, &config.tls_key)?;
    control_connector(config.control_ca_cert.as_deref())?;
    Ok(())
}

pub struct RunningRelayService {
    server: Server,
    refresh: tokio::task::JoinHandle<()>,
    usage: crate::usage::UsageWorker,
    retirement: Option<(PolicySyncSettings, tokio_tungstenite::Connector)>,
}

impl RunningRelayService {
    pub fn https_addr(&self) -> Option<SocketAddr> {
        self.server.https_addr()
    }

    pub fn captive_addr(&self) -> Option<SocketAddr> {
        self.server.http_addr()
    }

    pub fn quic_addr(&self) -> Option<SocketAddr> {
        self.server.quic_addr()
    }

    pub async fn shutdown(self) -> ServiceResult<()> {
        self.refresh.abort();
        let _ = self.refresh.await;
        self.server.shutdown().await?;
        retire(self.retirement).await;
        self.usage.shutdown().await;
        Ok(())
    }
}

pub async fn start_relay_service(config: RelayServiceConfig) -> ServiceResult<RunningRelayService> {
    config.validate()?;
    let server_tls = load_server_config(&config.tls_cert, &config.tls_key)?;
    let connector = control_connector(config.control_ca_cert.as_deref())?;
    let runtime = RelayPolicyRuntime::new(RelayPolicyState::new(config.limiter_burst)?);
    let mut sync_settings = PolicySyncSettings {
        process_boot_id: uuid::Uuid::new_v4(),
        public_url: config.public_url,
        metrics: None,
        boot_id: config.cluster_enabled.then(uuid::Uuid::new_v4),
        node_id: config.node_id.clone(),
        control_url: config.control_url,
        control_secret: config.control_secret,
        refresh_interval: config.policy_refresh_interval,
        reconnect_interval: config.reconnect_interval,
    };
    if let Some(boot) = sync_settings.boot_id {
        runtime.enable_cluster(config.node_id.clone(), boot);
    }
    let client = connect_and_sync(&sync_settings, connector.clone(), &runtime).await?;

    let mut relay = RelayConfig::new(config.captive_bind);
    relay.tls = Some(TlsConfig::new(
        config.https_bind,
        CertConfig::Manual {
            server_config: server_tls.clone(),
        },
    ));
    relay.access = Arc::new(runtime.clone());
    relay.forwarding = Arc::new(runtime.clone());

    let mut quic = QuicConfig::new(config.quic_bind);
    quic.server_config = Some(server_tls);
    let mut server_config = ServerConfig::default();
    server_config.relay = Some(relay);
    server_config.quic = Some(quic);
    let server = Server::spawn(server_config).await?;
    sync_settings.metrics = Some(server.metrics().server.clone());
    let https_addr = server
        .https_addr()
        .ok_or("Relay HTTPS listener is missing")?;
    let captive_addr = server
        .http_addr()
        .ok_or("iroh captive-portal listener is missing")?;
    let quic_addr = server.quic_addr().ok_or("Relay QUIC listener is missing")?;

    let usage = crate::usage::spawn(
        runtime.clone(),
        sync_settings.control_url.clone(),
        sync_settings.control_secret.clone(),
        connector.clone(),
        config.node_id,
        config.usage_dir,
        sync_settings.boot_id,
    );
    let retirement = sync_settings
        .boot_id
        .map(|_| (sync_settings.clone(), connector.clone()));
    let refresh = spawn_refresh_loop(client, sync_settings, connector, runtime);
    println!(
        "PAB Relay ready: https={https_addr} quic={quic_addr} captive_loopback={captive_addr}"
    );
    Ok(RunningRelayService {
        server,
        refresh,
        usage,
        retirement,
    })
}

pub async fn run_relay_service(config: RelayServiceConfig) -> ServiceResult<()> {
    run_relay_service_until(config, pab_service_host::stop_requested(None)).await
}
pub async fn run_relay_service_until(
    config: RelayServiceConfig,
    stop: impl std::future::Future<Output = ()>,
) -> ServiceResult<()> {
    let mut running = start_relay_service(config).await?;

    enum Stop {
        Signal,
        Server(Result<Result<(), iroh_relay::server::SupervisorError>, tokio::task::JoinError>),
    }
    let stop = tokio::select! {
        _ = stop => Stop::Signal,
        result = running.server.join() => Stop::Server(result),
    };
    running.refresh.abort();
    let _ = running.refresh.await;
    match stop {
        Stop::Signal => {
            running.server.shutdown().await?;
            retire(running.retirement).await;
        }
        Stop::Server(result) => result??,
    }
    running.usage.shutdown().await;
    Ok(())
}

async fn retire(retirement: Option<(PolicySyncSettings, tokio_tungstenite::Connector)>) {
    if let Some((settings, connector)) = retirement {
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let mut client = crate::RelayControlClient::connect(
                &settings.control_url,
                &settings.control_secret,
                connector,
            )
            .await?;
            let boot = settings.boot_id.expect("cluster retirement");
            client.cluster_hello(&settings.node_id, boot).await?;
            client.retire(boot).await
        })
        .await;
        if !matches!(result, Ok(Ok(()))) {
            tracing::warn!("Relay retirement was not confirmed; previous budgets expire normally");
        }
    }
}
