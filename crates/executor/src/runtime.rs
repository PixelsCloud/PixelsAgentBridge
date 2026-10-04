use std::{
    fs,
    sync::Arc,
    time::{Duration, SystemTime},
};

use pab_agent_core::{
    ControlConnectionPhase, DeviceHelloConfigError, DeviceNetworkConfigError,
    EndpointControlConfig, EndpointControlSupervisor, EndpointSecretError, ReconnectPolicy,
    ReconnectPolicyError, TlsConnectorError, read_endpoint_secret, tls_connector,
};
use pab_platform::{PlatformDetectionError, detect_native_execution_context};
use pab_protocol::{
    DEVICE_SESSION_SCHEMA_VERSION, DeviceHello, DeviceRef, EndpointKey, EndpointProofPrincipal,
};
use thiserror::Error;
use tokio::{sync::Semaphore, task::JoinSet};

use crate::{
    ExecutorConfig,
    credential::{DeviceCredential, DeviceCredentialError},
    session::DeviceSessionAcceptor,
    task_service::TaskService,
};
use crate::{
    network::ExecutorNetworkError,
    network::{bind_endpoint, watch_device_network},
};

pub async fn run_executor(config: ExecutorConfig) -> Result<(), ExecutorError> {
    config.validate()?;
    DeviceCredential::read(&config.task_database_file).await?;
    let heartbeat_path = config
        .task_database_file
        .with_file_name("executor-heartbeat.json");
    let secret = read_endpoint_secret(&config.endpoint_secret_file)?;
    let endpoint = bind_endpoint(&config, secret.clone()).await?;
    tracing::info!(endpoint = %endpoint.id(), "iroh endpoint ready");
    let extra_ca = config
        .control_ca_cert
        .as_ref()
        .map(fs::read)
        .transpose()
        .map_err(ExecutorError::ControlCaFile)?;
    let connector = tls_connector(extra_ca.as_deref())?;
    let execution_context = detect_native_execution_context()?;
    let device_ref = DeviceRef {
        tenant_id: config.tenant_id,
        device_id: config.device_id,
    };
    let network_updates = watch_device_network(
        &endpoint,
        device_ref,
        EndpointKey::new(*secret.public().as_bytes()),
    )?;
    let hello = DeviceHello {
        schema_version: DEVICE_SESSION_SCHEMA_VERSION,
        device_ref,
        execution_context: execution_context.clone(),
        agent_version: env!("CARGO_PKG_VERSION").to_owned(),
        observed_at_unix_ms: unix_millis(SystemTime::now())?,
    };
    let control = EndpointControlConfig {
        url: config.control_url,
        tenant_id: config.tenant_id,
        principal: EndpointProofPrincipal::Device {
            device_id: config.device_id,
        },
        operation_timeout: config.operation_timeout,
    };
    let supervisor =
        EndpointControlSupervisor::new(control, secret, connector, ReconnectPolicy::default())?
            .with_device_hello(hello.clone())?
            .with_device_network(network_updates)?
            .spawn();
    let peer_authorizer = supervisor.peer_authorizer();
    let task_service = TaskService::open(&config.task_database_file, device_ref, execution_context)
        .await
        .map_err(|error| ExecutorError::TaskService(error.to_string()))?;
    let session_acceptor = DeviceSessionAcceptor::new(
        peer_authorizer,
        config.task_database_file.clone(),
        device_ref,
        config.operation_timeout,
    )
    .with_task_service(task_service);
    let session_limit = Arc::new(Semaphore::new(64));
    let mut sessions = JoinSet::new();
    let mut status = supervisor.status();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(3));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    report_status(&status.borrow());
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if let Err(error) = write_heartbeat(&heartbeat_path, &status.borrow()) {
                    tracing::warn!(%error, "could not update executor heartbeat");
                }
            }
            signal = tokio::signal::ctrl_c() => {
                signal.map_err(ExecutorError::ShutdownSignal)?;
                sessions.abort_all();
                supervisor.shutdown().await?;
                endpoint.close().await;
                return Ok(());
            }
            changed = status.changed() => {
                if changed.is_err() {
                    endpoint.close().await;
                    return Err(ExecutorError::SupervisorClosed);
                }
                let current = status.borrow().clone();
                report_status(&current);
                if let Err(error) = write_heartbeat(&heartbeat_path, &current) {
                    tracing::warn!(%error, "could not update executor heartbeat");
                }
                if current.phase == ControlConnectionPhase::Stopped {
                    sessions.abort_all();
                    supervisor.shutdown().await?;
                    endpoint.close().await;
                    return Err(ExecutorError::SupervisorStopped(
                        current.last_failure.map(|failure| failure.detail)
                    ));
                }
            }
            incoming = endpoint.accept() => {
                match incoming {
                    Ok(Some(connection)) => {
                        let Ok(permit) = session_limit.clone().try_acquire_owned() else {
                            connection.close(b"too many device sessions");
                            continue;
                        };
                        let session_acceptor = session_acceptor.clone();
                        sessions.spawn(async move {
                            let _permit = permit;
                            session_acceptor.handle(connection).await
                        });
                    }
                    Ok(None) => {
                        sessions.abort_all();
                        supervisor.shutdown().await?;
                        return Err(ExecutorError::EndpointClosed);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "incoming connection failed; retrying in 3s");
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                }
            }
            completed = sessions.join_next(), if !sessions.is_empty() => {
                match completed {
                    Some(Ok(Err(error))) => tracing::warn!(%error, "device session failed"),
                    Some(Err(error)) if !error.is_cancelled() => {
                        tracing::error!(%error, "device session task failed");
                    }
                    _ => {}
                }
            }
        }
    }
}

fn write_heartbeat(
    path: &std::path::Path,
    status: &pab_agent_core::ControlConnectionStatus,
) -> std::io::Result<()> {
    let observed_at_unix_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_millis();
    let payload = serde_json::json!({
        "observed_at_unix_ms": observed_at_unix_ms,
        "control_phase": format!("{:?}", status.phase),
    });
    fs::write(path, payload.to_string())
}

fn report_status(status: &pab_agent_core::ControlConnectionStatus) {
    tracing::info!(
        phase = ?status.phase,
        generation = status.generation,
        failures = status.consecutive_failures,
        retry_ms = ?status.retry_in_ms,
        "control connection status"
    );
    if let Some(failure) = &status.last_failure {
        tracing::warn!(kind = ?failure.kind, detail = %failure.detail, "control connection failure");
    }
}

fn unix_millis(now: SystemTime) -> Result<i64, ExecutorError> {
    let value = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| ExecutorError::InvalidSystemTime)?
        .as_millis();
    i64::try_from(value).map_err(|_| ExecutorError::InvalidSystemTime)
}

#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error(transparent)]
    Config(#[from] crate::ExecutorConfigError),
    #[error(transparent)]
    EndpointSecret(#[from] EndpointSecretError),
    #[error(transparent)]
    DeviceCredential(#[from] DeviceCredentialError),
    #[error(transparent)]
    Network(#[from] ExecutorNetworkError),
    #[error("the control CA file could not be read: {0}")]
    ControlCaFile(std::io::Error),
    #[error(transparent)]
    Tls(#[from] TlsConnectorError),
    #[error(transparent)]
    Platform(#[from] PlatformDetectionError),
    #[error(transparent)]
    ReconnectPolicy(#[from] ReconnectPolicyError),
    #[error(transparent)]
    DeviceHello(#[from] DeviceHelloConfigError),
    #[error(transparent)]
    DeviceNetwork(#[from] DeviceNetworkConfigError),
    #[error("system time is outside the supported Unix timestamp range")]
    InvalidSystemTime,
    #[error("the shutdown signal listener failed: {0}")]
    ShutdownSignal(std::io::Error),
    #[error("the control supervisor status channel closed unexpectedly")]
    SupervisorClosed,
    #[error("the control supervisor stopped: {0:?}")]
    SupervisorStopped(Option<String>),
    #[error("the control supervisor task failed: {0}")]
    SupervisorTask(#[from] tokio::task::JoinError),
    #[error("the iroh endpoint closed unexpectedly")]
    EndpointClosed,
    #[error("the local task service failed to start: {0}")]
    TaskService(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_valid_native_device_hello_payload() {
        let context = detect_native_execution_context().unwrap();
        let hello = DeviceHello {
            schema_version: DEVICE_SESSION_SCHEMA_VERSION,
            device_ref: DeviceRef {
                tenant_id: pab_protocol::TenantId::from_u128(2),
                device_id: pab_protocol::DeviceId::from_u128(3),
            },
            execution_context: context,
            agent_version: env!("CARGO_PKG_VERSION").to_owned(),
            observed_at_unix_ms: unix_millis(SystemTime::now()).unwrap(),
        };

        assert_eq!(hello.schema_version, DEVICE_SESSION_SCHEMA_VERSION);
        assert!(hello.observed_at_unix_ms > 0);
        assert!(hello.execution_context.interpreter.is_none());
    }
}
