use std::{error::Error, net::SocketAddr, sync::Arc, time::Duration};

use iroh_relay::server::{CertConfig, QuicConfig, RelayConfig, Server, ServerConfig, TlsConfig};

use super::{
    RelayServiceConfig,
    sync::{PolicySyncSettings, connect_and_sync, spawn_refresh_loop},
    tls::{control_connector, load_server_config},
};
use crate::{RelayControlClientError, RelayPolicyRuntime, RelayPolicyState};

type ServiceResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

fn retry_initial_sync(error: &RelayControlClientError) -> bool {
    use pab_protocol::RelayControlErrorCode;
    use tokio_tungstenite::tungstenite::Error as WebSocketError;
    match error {
        RelayControlClientError::Server {
            code: RelayControlErrorCode::NodeBusy | RelayControlErrorCode::Internal,
            ..
        }
        | RelayControlClientError::Timeout
        | RelayControlClientError::Closed => true,
        RelayControlClientError::WebSocket(WebSocketError::Io(error)) => matches!(
            error.kind(),
            std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::NotConnected
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::Interrupted
                | std::io::ErrorKind::UnexpectedEof
        ),
        RelayControlClientError::WebSocket(WebSocketError::Http(response)) => {
            response.status().is_server_error()
        }
        _ => false,
    }
}

async fn initial_sync(
    settings: &PolicySyncSettings,
    connector: tokio_tungstenite::Connector,
    runtime: &RelayPolicyRuntime,
) -> Result<crate::RelayControlClient, RelayControlClientError> {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            match connect_and_sync(settings, connector.clone(), runtime).await {
                Ok(client) => return Ok(client),
                Err(error) if retry_initial_sync(&error) => {
                    tracing::warn!(%error, "Relay control is not ready during startup; retrying");
                    tokio::time::sleep(settings.reconnect_interval).await;
                }
                Err(error) => return Err(error),
            }
        }
    })
    .await
    .map_err(|_| RelayControlClientError::Timeout)?
}

pub fn check_relay_config(config: &RelayServiceConfig) -> ServiceResult<()> {
    config.validate()?;
    load_server_config(&config.tls_cert, &config.tls_key)?;
    control_connector(config.control_ca_cert.as_deref())?;
    Ok(())
}

pub struct RunningRelayService {
    server: Server,
    refresh: tokio::task::JoinHandle<()>,
    drain: tokio::task::JoinHandle<()>,
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
        self.drain.abort();
        let _ = self.drain.await;
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
    let client = initial_sync(&sync_settings, connector.clone(), &runtime).await?;

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
    let clients = server.relay_service().expect("Relay service").clients().clone();
    let drain_runtime = runtime.clone();
    let drain = tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if !drain_runtime.node_flags().1 {
                // Repeating also catches an admission that raced the state change.
                clients.shutdown().await;
            }
        }
    });
    let refresh = spawn_refresh_loop(client, sync_settings, connector, runtime);
    println!(
        "PAB Relay ready: https={https_addr} quic={quic_addr} captive_loopback={captive_addr}"
    );
    Ok(RunningRelayService {
        server,
        refresh,
        drain,
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
    tokio::pin!(stop);
    let mut running = tokio::select! {
        running = start_relay_service(config) => running?,
        _ = &mut stop => return Ok(()),
    };

    enum Stop {
        Signal,
        Server(Result<Result<(), iroh_relay::server::SupervisorError>, tokio::task::JoinError>),
    }
    let stop = tokio::select! {
        _ = &mut stop => Stop::Signal,
        result = running.server.join() => Stop::Server(result),
    };
    running.refresh.abort();
    let _ = running.refresh.await;
    running.drain.abort();
    let _ = running.drain.await;
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

#[cfg(test)]
mod startup_tests {
    use super::*;
    use pab_protocol::RelayControlErrorCode;

    #[test]
    fn startup_retries_temporary_failures_and_rejects_identity_or_tls_errors() {
        let server = |code| RelayControlClientError::Server {
            code,
            message: "test rejection".into(),
        };
        assert!(retry_initial_sync(&server(RelayControlErrorCode::NodeBusy)));
        assert!(retry_initial_sync(&server(RelayControlErrorCode::Internal)));
        assert!(!retry_initial_sync(&server(RelayControlErrorCode::InvalidMessage)));
        assert!(!retry_initial_sync(&server(RelayControlErrorCode::InvalidPolicyVersion)));
        for (kind, retry) in [
            (std::io::ErrorKind::ConnectionRefused, true),
            (std::io::ErrorKind::InvalidData, false),
            (std::io::ErrorKind::PermissionDenied, false),
        ] {
            let error = RelayControlClientError::WebSocket(
                tokio_tungstenite::tungstenite::Error::Io(std::io::Error::new(kind, "test")),
            );
            assert_eq!(retry_initial_sync(&error), retry);
        }
    }

    #[tokio::test]
    async fn stopping_during_initial_connection_does_not_wait_for_startup_timeout() {
        let directory = tempfile::tempdir().unwrap();
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert = directory.path().join("cert.pem");
        let key = directory.path().join("key.pem");
        std::fs::write(&cert, certified.cert.pem()).unwrap();
        std::fs::write(&key, certified.signing_key.serialize_pem()).unwrap();
        let control = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = control.local_addr().unwrap().port();
        drop(control);
        let config = RelayServiceConfig {
            cluster_enabled: true,
            public_url: None,
            node_id: "startup-test".into(),
            usage_dir: directory.path().join("usage"),
            control_url: format!("wss://localhost:{port}/relay-control"),
            control_secret: "isolated startup test secret 0123456789".into(),
            control_ca_cert: Some(cert.clone()),
            tls_cert: cert,
            tls_key: key,
            https_bind: "127.0.0.1:0".parse().unwrap(),
            captive_bind: "127.0.0.1:0".parse().unwrap(),
            quic_bind: "127.0.0.1:0".parse().unwrap(),
            policy_refresh_interval: Duration::from_secs(1),
            reconnect_interval: Duration::from_millis(100),
            limiter_burst: Duration::from_millis(100),
        };
        tokio::time::timeout(
            Duration::from_secs(2),
            run_relay_service_until(config, async {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }),
        )
        .await
        .expect("stop must cancel initial connection retries")
        .unwrap();
    }
}
