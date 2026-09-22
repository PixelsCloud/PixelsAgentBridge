use std::{fs, time::SystemTime};

use pab_agent_core::{
    ControlConnectionPhase, DeviceHelloConfigError, DeviceNetworkConfigError,
    EndpointControlConfig, EndpointControlSupervisor, ReconnectPolicy, ReconnectPolicyError,
    TlsConnectorError, tls_connector,
};
use pab_platform::{PlatformDetectionError, detect_native_execution_context};
use pab_protocol::{
    DEVICE_SESSION_SCHEMA_VERSION, DeviceHello, DeviceRef, EndpointKey, EndpointProofPrincipal,
};
use thiserror::Error;

use crate::{ExecutorConfig, identity::EndpointSecretError, identity::read_endpoint_secret};
use crate::{
    network::ExecutorNetworkError,
    network::{bind_endpoint, watch_device_network},
};

pub async fn run_executor(config: ExecutorConfig) -> Result<(), ExecutorError> {
    config.validate()?;
    let secret = read_endpoint_secret(&config.endpoint_secret_file)?;
    let endpoint = bind_endpoint(&config, secret.clone()).await?;
    eprintln!("pab-executor: iroh_endpoint={}", endpoint.id());
    let extra_ca = config
        .control_ca_cert
        .as_ref()
        .map(fs::read)
        .transpose()
        .map_err(ExecutorError::ControlCaFile)?;
    let connector = tls_connector(extra_ca.as_deref())?;
    let execution_context = detect_native_execution_context()?;
    let device_ref = DeviceRef {
        deployment_id: config.deployment_id,
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
        execution_context,
        agent_version: env!("CARGO_PKG_VERSION").to_owned(),
        observed_at_unix_ms: unix_millis(SystemTime::now())?,
    };
    let control = EndpointControlConfig {
        url: config.control_url,
        deployment_id: config.deployment_id,
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
    let mut status = supervisor.status();
    report_status(&status.borrow());
    loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal.map_err(ExecutorError::ShutdownSignal)?;
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
                if current.phase == ControlConnectionPhase::Stopped {
                    supervisor.shutdown().await?;
                    endpoint.close().await;
                    return Err(ExecutorError::SupervisorStopped(
                        current.last_failure.map(|failure| failure.detail)
                    ));
                }
            }
        }
    }
}

fn report_status(status: &pab_agent_core::ControlConnectionStatus) {
    eprintln!(
        "pab-executor: control={:?} generation={} failures={} retry_ms={:?}",
        status.phase, status.generation, status.consecutive_failures, status.retry_in_ms
    );
    if let Some(failure) = &status.last_failure {
        eprintln!(
            "pab-executor: last_failure={:?}: {}",
            failure.kind, failure.detail
        );
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
                deployment_id: pab_protocol::DeploymentId::from_u128(1),
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
