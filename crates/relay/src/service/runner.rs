use std::{error::Error, net::SocketAddr, sync::Arc};

use iroh_relay::server::{CertConfig, QuicConfig, RelayConfig, Server, ServerConfig, TlsConfig};

use super::{
    RelayServiceConfig,
    sync::{PolicySyncSettings, connect_and_sync, spawn_refresh_loop},
    tls::{control_connector, load_server_config},
};
use crate::{RelayPolicyRuntime, RelayPolicyState};

type ServiceResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub struct RunningRelayService {
    server: Server,
    refresh: tokio::task::JoinHandle<()>,
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
        Ok(())
    }
}

pub async fn start_relay_service(config: RelayServiceConfig) -> ServiceResult<RunningRelayService> {
    config.validate()?;
    let server_tls = load_server_config(&config.tls_cert, &config.tls_key)?;
    let connector = control_connector(config.control_ca_cert.as_deref())?;
    let runtime = RelayPolicyRuntime::new(RelayPolicyState::new(config.limiter_burst)?);
    let sync_settings = PolicySyncSettings {
        control_url: config.control_url,
        control_secret: config.control_secret,
        refresh_interval: config.policy_refresh_interval,
        reconnect_interval: config.reconnect_interval,
    };
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
    let https_addr = server
        .https_addr()
        .ok_or("Relay HTTPS listener is missing")?;
    let captive_addr = server
        .http_addr()
        .ok_or("iroh captive-portal listener is missing")?;
    let quic_addr = server.quic_addr().ok_or("Relay QUIC listener is missing")?;

    let refresh = spawn_refresh_loop(client, sync_settings, connector, runtime);
    println!(
        "PAB Relay ready: https={https_addr} quic={quic_addr} captive_loopback={captive_addr}"
    );
    Ok(RunningRelayService { server, refresh })
}

pub async fn run_relay_service(config: RelayServiceConfig) -> ServiceResult<()> {
    let mut running = start_relay_service(config).await?;

    enum Stop {
        Signal,
        Server(Result<Result<(), iroh_relay::server::SupervisorError>, tokio::task::JoinError>),
    }
    let stop = tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            signal?;
            Stop::Signal
        }
        result = running.server.join() => Stop::Server(result),
    };
    running.refresh.abort();
    let _ = running.refresh.await;
    match stop {
        Stop::Signal => running.server.shutdown().await?,
        Stop::Server(result) => result??,
    }
    Ok(())
}
