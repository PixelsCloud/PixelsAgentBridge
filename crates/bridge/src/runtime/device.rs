use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use pab_protocol::DeviceRef;
use pab_transport::ConnectionPath;
use tokio::sync::{Mutex, RwLock, watch};

use crate::{
    AuthenticatedDeviceConnection, BridgeClient, BridgeConfig, BridgeConnector, BridgeError,
};

use super::{DeviceConnectionPhase, RuntimeError, RuntimeEventKind, RuntimeInner, duration_millis};

pub(super) struct DeviceSession {
    device_ref: DeviceRef,
    connection: RwLock<Option<Arc<AuthenticatedDeviceConnection>>>,
    connect_lock: Mutex<()>,
    manually_disconnected: AtomicBool,
    runtime: Arc<RuntimeInner>,
}

impl DeviceSession {
    pub(super) async fn cached_connection(&self) -> Option<Arc<AuthenticatedDeviceConnection>> {
        self.connection.read().await.clone()
    }
    #[cfg(test)]
    pub(super) async fn set_test_connection(&self, connection: AuthenticatedDeviceConnection) {
        *self.connection.write().await = Some(Arc::new(connection));
    }

    pub(super) fn new(device_ref: DeviceRef, runtime: Arc<RuntimeInner>) -> Self {
        Self {
            device_ref,
            connection: RwLock::new(None),
            connect_lock: Mutex::new(()),
            manually_disconnected: AtomicBool::new(false),
            runtime,
        }
    }

    pub(super) fn resume(&self) {
        self.manually_disconnected.store(false, Ordering::Release);
    }

    pub(super) async fn selected_path(&self) -> Option<ConnectionPath> {
        if self.manually_disconnected.load(Ordering::Acquire) {
            return None;
        }

        self.connection
            .read()
            .await
            .as_ref()
            .and_then(|connection| connection.connection().selected_path())
    }

    pub(super) async fn disconnect(&self) {
        self.manually_disconnected.store(true, Ordering::Release);
        if let Some(connection) = self.connection.write().await.take() {
            connection.as_ref().clone().close();
        }
        self.runtime.publish(RuntimeEventKind::DeviceConnection {
            device_ref: self.device_ref,
            phase: DeviceConnectionPhase::Disconnected,
            retry_in_ms: None,
            message: None,
        });
    }

    fn ensure_enabled(&self) -> Result<(), RuntimeError> {
        if self.manually_disconnected.load(Ordering::Acquire) {
            return Err(RuntimeError::DeviceDisconnected);
        }
        Ok(())
    }

    pub(super) async fn connection(
        &self,
    ) -> Result<Arc<AuthenticatedDeviceConnection>, RuntimeError> {
        self.ensure_enabled()?;
        self.runtime.wait_account_ready().await?;
        if let Some(connection) = self.connection.read().await.as_ref() {
            return Ok(Arc::clone(connection));
        }
        let _guard = self.connect_lock.lock().await;
        self.ensure_enabled()?;
        if let Some(connection) = self.connection.read().await.as_ref() {
            return Ok(Arc::clone(connection));
        }
        let mut availability = self.runtime.availability.clone();
        let connector = loop {
            self.ensure_enabled()?;
            match availability.borrow().clone() {
                BridgeAvailability::Connecting => {
                    self.runtime.publish(RuntimeEventKind::DeviceConnection {
                        device_ref: self.device_ref,
                        phase: DeviceConnectionPhase::WaitingForBridge,
                        retry_in_ms: None,
                        message: None,
                    });
                }
                BridgeAvailability::Connected(connector) => break *connector,
                BridgeAvailability::Stopped(message) => {
                    return Err(RuntimeError::BridgeUnavailable(message));
                }
            }
            let mut shutdown = self.runtime.shutdown.clone();
            tokio::select! {
                changed = availability.changed() => {
                    changed.map_err(|_| RuntimeError::BridgeUnavailable("Bridge supervisor stopped".to_owned()))?;
                }
                changed = shutdown.changed() => {
                    let _ = changed;
                    return Err(RuntimeError::ShuttingDown);
                }
            }
        };
        loop {
            self.ensure_enabled()?;
            self.runtime.publish(RuntimeEventKind::DeviceConnection {
                device_ref: self.device_ref,
                phase: DeviceConnectionPhase::Connecting,
                retry_in_ms: None,
                message: None,
            });
            let password = match self.runtime.passwords.load_password(self.device_ref).await {
                Ok(password) => password,
                Err(error) => {
                    self.runtime.publish(RuntimeEventKind::DeviceConnection {
                        device_ref: self.device_ref,
                        phase: DeviceConnectionPhase::Disconnected,
                        retry_in_ms: None,
                        message: Some(error.to_string()),
                    });
                    return Err(error.into());
                }
            };
            match connector.connect_device(self.device_ref, password).await {
                Ok(connection) => {
                    if self.manually_disconnected.load(Ordering::Acquire) {
                        connection.close();
                        return Err(RuntimeError::DeviceDisconnected);
                    }
                    let connection = Arc::new(connection);
                    *self.connection.write().await = Some(Arc::clone(&connection));
                    self.runtime.publish(RuntimeEventKind::DeviceConnection {
                        device_ref: self.device_ref,
                        phase: DeviceConnectionPhase::Connected,
                        retry_in_ms: None,
                        message: None,
                    });
                    return Ok(connection);
                }
                Err(error) if error.is_recoverable_connection() => {
                    tracing::warn!(device_id = %self.device_ref.device_id, %error, "device connection attempt failed, retrying");
                    self.runtime.publish(RuntimeEventKind::DeviceConnection {
                        device_ref: self.device_ref,
                        phase: DeviceConnectionPhase::Retrying,
                        retry_in_ms: Some(duration_millis(self.runtime.retry_interval)),
                        message: Some(error.to_string()),
                    });
                    self.runtime
                        .wait_or_shutdown(self.runtime.retry_interval)
                        .await?;
                }
                Err(error) => {
                    self.runtime.publish(RuntimeEventKind::DeviceConnection {
                        device_ref: self.device_ref,
                        phase: DeviceConnectionPhase::Disconnected,
                        retry_in_ms: None,
                        message: Some(error.to_string()),
                    });
                    return Err(error.into());
                }
            }
        }
    }

    pub(super) async fn recover(
        &self,
        failed: &Arc<AuthenticatedDeviceConnection>,
        error: &BridgeError,
    ) -> Result<(), RuntimeError> {
        self.ensure_enabled()?;
        let mut connection = self.connection.write().await;
        if connection
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, failed))
        {
            connection.take();
            failed.as_ref().clone().close();
        }
        drop(connection);
        self.runtime.publish(RuntimeEventKind::DeviceConnection {
            device_ref: self.device_ref,
            phase: DeviceConnectionPhase::Retrying,
            retry_in_ms: Some(duration_millis(self.runtime.retry_interval)),
            message: Some(error.to_string()),
        });
        self.runtime
            .wait_or_shutdown(self.runtime.retry_interval)
            .await
    }
}

#[derive(Clone)]
pub(super) enum BridgeAvailability {
    Connecting,
    Connected(Box<BridgeConnector>),
    Stopped(String),
}

pub(super) async fn run_bridge_supervisor(
    config: BridgeConfig,
    retry_interval: Duration,
    availability: watch::Sender<BridgeAvailability>,
    mut shutdown: watch::Receiver<bool>,
    runtime: Arc<RuntimeInner>,
) {
    loop {
        match BridgeClient::connect(config.clone()).await {
            Ok(bridge) => {
                let _ =
                    availability.send(BridgeAvailability::Connected(Box::new(bridge.connector())));
                runtime.publish(RuntimeEventKind::BridgeConnected);
                let mut control = bridge.control_status();
                loop {
                    {
                        let status = control.borrow().clone();
                        let mut presence =
                            runtime.presence.lock().unwrap_or_else(|e| e.into_inner());
                        presence.control_phase = match status.phase {
                            pab_agent_core::ControlConnectionPhase::Disconnected => "disconnected",
                            pab_agent_core::ControlConnectionPhase::Connecting => "connecting",
                            pab_agent_core::ControlConnectionPhase::Authenticated => {
                                "authenticated"
                            }
                            pab_agent_core::ControlConnectionPhase::Reconnecting => "reconnecting",
                            pab_agent_core::ControlConnectionPhase::Stopped => "stopped",
                        }
                        .to_owned();
                        presence.last_error = status.last_failure.map(|failure| failure.detail);
                        presence.control_changed_at_unix_ms = status.changed_at_unix_ms;
                        presence.control_generation = status.generation;
                        presence.control_consecutive_failures = status.consecutive_failures;
                        presence.control_retry_in_ms = status.retry_in_ms;
                    }
                    tokio::select! {
                        _ = shutdown.changed() => break,
                        changed = control.changed() => if changed.is_err() { break; },
                    }
                }
                let _ = availability.send(BridgeAvailability::Stopped(
                    "Bridge Runtime is shutting down".to_owned(),
                ));
                if let Err(error) = bridge.shutdown().await {
                    runtime.publish(RuntimeEventKind::BridgeStopped {
                        message: error.to_string(),
                    });
                }
                return;
            }
            Err(error) => {
                {
                    let mut presence = runtime.presence.lock().unwrap_or_else(|e| e.into_inner());
                    presence.control_phase = "retrying".to_owned();
                    presence.last_error = Some(error.to_string());
                }
                runtime.publish(RuntimeEventKind::BridgeRetrying {
                    retry_in_ms: duration_millis(retry_interval),
                    message: error.to_string(),
                });
                tokio::select! {
                    _ = tokio::time::sleep(retry_interval) => {}
                    changed = shutdown.changed() => {
                        let _ = changed;
                        let _ = availability.send(BridgeAvailability::Stopped(
                            "Bridge Runtime is shutting down".to_owned(),
                        ));
                        return;
                    }
                }
            }
        }
    }
}
