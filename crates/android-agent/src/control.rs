//! Android lifecycle adapter around the existing signed control supervisor.
//! Optional relay configuration enables the authenticated mobile execution endpoint.

use std::time::Duration;

use iroh_base::SecretKey;
use pab_agent_core::{
    ControlConnectionPhase, EndpointControlConfig, EndpointControlSupervisor,
    EndpointControlSupervisorHandle, OpenRegistrationKind, ReconnectPolicy, register_open_endpoint,
    tls_connector,
};
use pab_protocol::{EndpointProofPrincipal, EndpointRegistrationResult};
use serde_json::{Value, json};
use tokio::runtime::Runtime;

use crate::config::AndroidConfig;

pub struct ControlAgent {
    // Drop the supervisor before its runtime, even if construction later changes.
    supervisor: Option<EndpointControlSupervisorHandle>,
    runtime: Runtime,
    registration: EndpointRegistrationResult,
    peer: Option<crate::peer::PeerRuntime>,
}

impl ControlAgent {
    pub fn rename_device(
        config: &AndroidConfig,
        key: &[u8],
        registration: &str,
        name: &str,
    ) -> Result<Value, &'static str> {
        config.validate()?;
        if name.trim() != name
            || name.is_empty()
            || name.chars().count() > 80
            || name.chars().any(char::is_control)
        {
            return Err("invalid_device_name");
        }
        let registration: EndpointRegistrationResult =
            serde_json::from_str(registration).map_err(|_| "invalid_registration")?;
        let EndpointRegistrationResult::Device {
            tenant_id,
            device_id,
            ..
        } = registration
        else {
            return Err("invalid_registration");
        };
        let connector = tls_connector(config.trusted_ca_pem.as_deref().map(str::as_bytes))
            .map_err(|_| "invalid_ca")?;
        runtime()?.block_on(async {
            let mut connection = pab_agent_core::AuthenticatedControlConnection::connect(
                &EndpointControlConfig {
                    url: config.control_url.clone(),
                    tenant_id,
                    principal: EndpointProofPrincipal::Device { device_id },
                    operation_timeout: Duration::from_secs(8),
                },
                &secret(key)?,
                connector,
            )
            .await
            .map_err(|_| "device_rename_failed")?;
            connection
                .rename_own_device(name, Duration::from_secs(8))
                .await
                .map_err(|error| match error {
                    pab_agent_core::EndpointControlError::Server {
                        code: pab_protocol::ControlErrorCode::RateLimited,
                        ..
                    } => "device_rename_rate_limited",
                    _ => "device_rename_failed",
                })?;
            Ok(json!({"name":name}))
        })
    }
    /// A stored registration avoids registering an already claimed device again.
    /// The caller persists the returned registration before calling `start`.
    pub fn register(config: &AndroidConfig, key: &[u8]) -> Result<Value, &'static str> {
        config.validate()?;
        let secret = secret(key)?;
        let connector = tls_connector(config.trusted_ca_pem.as_deref().map(str::as_bytes))
            .map_err(|_| "invalid_ca")?;
        let runtime = runtime()?;
        let result = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(30), async {
                register_open_endpoint(
                    &config.control_url,
                    &secret,
                    OpenRegistrationKind::Device {
                        name: config.device_name.clone(),
                    },
                    connector,
                    Duration::from_secs(8),
                )
                .await
            })
            .await
        });
        match result {
            Ok(Ok(result @ EndpointRegistrationResult::Device { .. })) => Ok(json!(result)),
            Ok(Ok(_)) => Err("invalid_registration"),
            Ok(Err(_)) => Err("registration_failed"),
            Err(_) => Err("registration_timeout"),
        }
    }

    pub fn start(
        config: &AndroidConfig,
        key: &[u8],
        registration: &str,
    ) -> Result<Self, &'static str> {
        Self::start_with_options(config, key, registration, None)
    }

    pub fn start_with_options(
        config: &AndroidConfig,
        key: &[u8],
        registration: &str,
        options: Option<crate::peer::RuntimeOptions>,
    ) -> Result<Self, &'static str> {
        config.validate()?;
        let secret = secret(key)?;
        if registration.len() > 4096 {
            return Err("invalid_registration");
        }
        let registration: EndpointRegistrationResult =
            serde_json::from_str(registration).map_err(|_| "invalid_registration")?;
        let EndpointRegistrationResult::Device {
            tenant_id,
            device_id,
            endpoint_key,
            ..
        } = registration
        else {
            return Err("invalid_registration");
        };
        if endpoint_key.as_bytes() != secret.public().as_bytes() {
            return Err("identity_mismatch");
        }
        let connector = tls_connector(config.trusted_ca_pem.as_deref().map(str::as_bytes))
            .map_err(|_| "invalid_ca")?;
        let runtime = runtime()?;
        let digest_key =
            blake3::derive_key("pixels android operation digest v1", &secret.to_bytes());
        let device = pab_protocol::DeviceRef {
            tenant_id,
            device_id,
        };
        let execution = options.as_ref().map(crate::peer::context).transpose()?;
        let endpoint = if config.relay_urls.is_empty() {
            None
        } else {
            if options.is_none() {
                return Err("runtime_options_required");
            }
            Some(runtime.block_on(crate::peer::bind(config, secret.clone()))?)
        };
        let supervisor = {
            let _guard = runtime.enter();
            let mut supervisor = EndpointControlSupervisor::new(
                EndpointControlConfig {
                    url: config.control_url.clone(),
                    tenant_id,
                    principal: EndpointProofPrincipal::Device { device_id },
                    operation_timeout: Duration::from_secs(8),
                },
                secret,
                connector,
                ReconnectPolicy::default(),
            )
            .map_err(|_| "invalid_reconnect_policy")?;
            if let (Some(endpoint), Some(execution)) = (&endpoint, &execution) {
                supervisor = supervisor
                    .with_device_hello(pab_protocol::DeviceHello {
                        schema_version: pab_protocol::DEVICE_SESSION_SCHEMA_VERSION,
                        device_ref: device,
                        execution_context: execution.clone(),
                        agent_version: env!("CARGO_PKG_VERSION").into(),
                        observed_at_unix_ms: crate::peer::now_ms(),
                    })
                    .map_err(|_| "invalid_hello")?
                    .with_optional_device_hello()
                    .with_device_network(crate::peer::network(endpoint, device))
                    .map_err(|_| "invalid_network")?
                    .with_relay_directory();
            }
            supervisor.spawn()
        };
        let peer = if let (Some(endpoint), Some(execution), Some(options)) =
            (endpoint, execution, options)
        {
            Some(runtime.block_on(crate::peer::PeerRuntime::start(
                endpoint,
                &supervisor,
                device,
                execution,
                options,
                digest_key,
            ))?)
        } else {
            None
        };
        Ok(Self {
            peer,
            supervisor: Some(supervisor),
            runtime,
            registration,
        })
    }

    pub fn update_password(
        &self,
        credential: pab_device_session::DevicePassword,
    ) -> Result<(), &'static str> {
        if let Some(peer) = &self.peer {
            peer.update_password(credential)?;
        }
        Ok(())
    }

    pub fn status(&self) -> Value {
        let status = self
            .supervisor
            .as_ref()
            .expect("running supervisor")
            .status();
        let current = status.borrow();
        let phase = match current.phase {
            ControlConnectionPhase::Disconnected => "disconnected",
            ControlConnectionPhase::Connecting => "connecting",
            ControlConnectionPhase::Authenticated => "authenticated",
            ControlConnectionPhase::Reconnecting => "reconnecting",
            ControlConnectionPhase::Stopped => "stopped",
        };
        let code = match &self.registration {
            EndpointRegistrationResult::Device { device_code, .. } => device_code.to_string(),
            _ => unreachable!("validated device registration"),
        };
        // Failure details may contain server-controlled text. Only return fixed categories.
        json!({ "phase": phase, "device_code": code, "generation": current.generation,
            "remote_ready": self.peer.is_some() && current.phase == ControlConnectionPhase::Authenticated,
            "failures": current.consecutive_failures, "retry_in_ms": current.retry_in_ms,
            "error": current.last_failure.as_ref().map(|error| format!("{:?}", error.kind).to_lowercase()) })
    }

    pub fn poll_platform(&self) -> Result<Value, &'static str> {
        match &self.peer {
            Some(peer) => self.runtime.block_on(async {
                if let Some(files) = &peer.files {
                    let command = files.authorization.poll().await;
                    if !command.is_null() {
                        return Ok(command);
                    }
                }
                if let Some(service) = &peer.execution {
                    let command = service.sdk.poll().await;
                    if !command.is_null() {
                        return Ok(command);
                    }
                }
                let mobile = peer.broker.poll().await?;
                if !mobile.is_null() {
                    return Ok(mobile);
                }
                if let Some(service) = &peer.system {
                    let system = service.sdk.poll().await;
                    if !system.is_null() {
                        return Ok(system);
                    }
                }
                if let Some(service) = &peer.documents {
                    let command = service.sdk.poll().await;
                    if !command.is_null() {
                        return Ok(command);
                    }
                }
                Ok(match &peer.files {
                    Some(files) => files.storage.poll().await,
                    None => Value::Null,
                })
            }),
            None => Ok(Value::Null),
        }
    }
    pub fn complete_platform(
        &self,
        ticket: uuid::Uuid,
        output: Value,
        jpeg: Vec<u8>,
    ) -> Result<(), &'static str> {
        let peer = self.peer.as_ref().ok_or("transport_unavailable")?;
        self.runtime.block_on(async {
            if let Some(files) = &peer.files
                && files.authorization.complete(ticket, output.clone()).await
            {
                return Ok(());
            }
            if let Some(service) = &peer.execution
                && service.sdk.complete(ticket, output.clone()).await
            {
                return Ok(());
            }
            if let Some(service) = &peer.documents
                && service.sdk.complete(ticket, output.clone()).await
            {
                return Ok(());
            }
            if let Some(service) = &peer.system
                && service.sdk.complete(ticket, output.clone()).await
            {
                return Ok(());
            }
            if let Some(files) = &peer.files
                && files.storage.complete(ticket, output.clone()).await
            {
                return Ok(());
            }
            peer.broker.complete(ticket, output, jpeg).await
        })
    }
    pub fn stop(mut self) {
        if let Some(peer) = self.peer.take() {
            self.runtime.block_on(peer.stop());
        }
        if let Some(supervisor) = self.supervisor.take() {
            // A connecting supervisor is bounded by its operation timeout. Drop the
            // runtime after this deadline to cancel any remaining connection work.
            self.runtime.block_on(async {
                let _ = tokio::time::timeout(Duration::from_secs(10), supervisor.shutdown()).await;
            });
        }
        self.runtime.shutdown_timeout(Duration::from_secs(1));
    }
}

fn runtime() -> Result<Runtime, &'static str> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(2)
        .thread_name("pixels-control")
        .enable_all()
        .build()
        .map_err(|_| "runtime_unavailable")
}

fn secret(bytes: &[u8]) -> Result<SecretKey, &'static str> {
    let bytes: &[u8; 32] = bytes.try_into().map_err(|_| "invalid_identity")?;
    let key = zeroize::Zeroizing::new(*bytes);
    Ok(SecretKey::from_bytes(&key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pab_protocol::{DeviceCode, DeviceId, EndpointKey, TenantId};

    #[test]
    fn rejects_wrong_seed_and_registration_before_connecting() {
        let config = AndroidConfig {
            schema_version: 1,
            control_url: "wss://localhost:1/control".into(),
            device_name: "Android".into(),
            trusted_ca_pem: None,
            relay_urls: Vec::new(),
            #[cfg(debug_assertions)]
            relay_only_for_testing: false,
        };
        let key = SecretKey::generate();
        let registration = json!(EndpointRegistrationResult::Device {
            tenant_id: TenantId::from_u128(1),
            device_id: DeviceId::from_u128(2),
            device_code: DeviceCode::new(123456789).unwrap(),
            endpoint_key: EndpointKey::new(*key.public().as_bytes()),
        })
        .to_string();
        assert!(matches!(
            ControlAgent::start(&config, &[1; 31], &registration),
            Err("invalid_identity")
        ));
        assert!(matches!(
            ControlAgent::start(&config, &[1; 32], &registration),
            Err("identity_mismatch")
        ));
        assert!(matches!(
            ControlAgent::start(&config, &key.to_bytes(), "{}"),
            Err("invalid_registration")
        ));
    }

    #[test]
    fn reconnecting_control_stops_without_leaving_a_runtime() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let config = AndroidConfig {
            schema_version: 1,
            control_url: format!("wss://127.0.0.1:{port}/control"),
            device_name: "Android".into(),
            trusted_ca_pem: None,
            relay_urls: Vec::new(),
            #[cfg(debug_assertions)]
            relay_only_for_testing: false,
        };
        let key = SecretKey::generate();
        let registration = json!(EndpointRegistrationResult::Device {
            tenant_id: TenantId::from_u128(1),
            device_id: DeviceId::from_u128(2),
            device_code: DeviceCode::new(123456789).unwrap(),
            endpoint_key: EndpointKey::new(*key.public().as_bytes()),
        })
        .to_string();
        let agent = ControlAgent::start(&config, &key.to_bytes(), &registration).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while agent.status()["phase"] != "reconnecting" {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(agent.status()["generation"], 0);
        agent.stop();
    }
}
