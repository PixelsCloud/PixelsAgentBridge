use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
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
    connection_mode: AtomicU8,
    runtime: Arc<RuntimeInner>,
    file_profile: bool,
    usage_session: String,
}

impl DeviceSession {
    pub(super) async fn sample_usage(&self, scope: Option<&str>) {
        let connection = self.connection.read().await;
        let id = connection
            .as_ref()
            .filter(|c| c.connection().selected_path().is_some())
            .map(|c| c.authenticated_at_unix_ms());
        if let Err(error) = self
            .runtime
            .store
            .sample_connection_traffic(
                &self.usage_session,
                self.device_ref.device_id,
                id,
                scope,
                connection.as_ref().map(|c| c.connection().direct_traffic()),
            )
            .await
        {
            tracing::warn!(%error,"connection usage sample unavailable");
        }
    }
    pub(super) async fn cached_connection(&self) -> Option<Arc<AuthenticatedDeviceConnection>> {
        self.connection.read().await.clone()
    }
    pub(super) async fn close_for_account_change(&self) {
        if let Some(connection) = self.connection.write().await.take() {
            self.finish_usage(&connection).await;
            connection.as_ref().clone().close();
            self.publish(RuntimeEventKind::DeviceConnection {
                device_ref: self.device_ref,
                phase: DeviceConnectionPhase::Disconnected,
                retry_in_ms: None,
                message: Some("login required".into()),
            });
        }
    }
    #[cfg(test)]
    pub(super) async fn set_test_connection(&self, connection: AuthenticatedDeviceConnection) {
        *self.connection.write().await = Some(Arc::new(connection));
    }

    pub(super) fn new(device_ref: DeviceRef, runtime: Arc<RuntimeInner>) -> Self {
        Self {
            usage_session: runtime.session_id.clone(),
            file_profile: false,
            device_ref,
            connection: RwLock::new(None),
            connect_lock: Mutex::new(()),
            manually_disconnected: AtomicBool::new(false),
            connection_mode: AtomicU8::new(0),
            runtime,
        }
    }

    pub(super) fn new_file(device: DeviceRef, runtime: Arc<RuntimeInner>, relay: bool) -> Self {
        let mut session = Self::new(device,runtime);
        session.file_profile = true;
        session.usage_session = format!("{}:files:{}",session.runtime.session_id,u8::from(relay));
        session.connection_mode.store(u8::from(relay),Ordering::Release);
        session
    }
    fn publish(&self, event: RuntimeEventKind) {
        // A file profile cannot overwrite the shared desktop/terminal status.
        if !self.file_profile {self.runtime.publish(event);}
    }

    pub(super) fn resume(&self) {
        self.manually_disconnected.store(false, Ordering::Release);
    }

    pub(super) fn connection_mode(&self) -> crate::ConnectionMode {
        if self.connection_mode.load(Ordering::Acquire) == 1 { crate::ConnectionMode::Relay } else { crate::ConnectionMode::Auto }
    }

    pub(super) async fn set_connection_mode(&self, mode: crate::ConnectionMode) {
        let mut connection = self.connection.write().await;
        if self.connection_mode() == mode { return; }
        self.connection_mode.store(u8::from(mode == crate::ConnectionMode::Relay), Ordering::Release);
        if let Some(previous) = connection.take() {
            self.finish_usage(&previous).await;
            previous.as_ref().clone().close();
        }
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
            self.finish_usage(&connection).await;
            connection.as_ref().clone().close();
        }
        let _ = self
            .runtime
            .store
            .sample_connection(
                &self.usage_session,
                self.device_ref.device_id,
                None,
                None,
            )
            .await;
        self.publish(RuntimeEventKind::DeviceConnection {
            device_ref: self.device_ref,
            phase: DeviceConnectionPhase::Disconnected,
            retry_in_ms: None,
            message: None,
        });
    }

    async fn finish_usage(&self, connection: &AuthenticatedDeviceConnection) {
        if let Err(error) = self
            .runtime
            .store
            .sample_connection_traffic(
                &self.runtime.session_id,
                self.device_ref.device_id,
                None,
                None,
                Some(connection.connection().direct_traffic()),
            )
            .await
        {
            tracing::warn!(%error, "final connection usage sample unavailable");
        }
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
                    self.publish(RuntimeEventKind::DeviceConnection {
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
            self.publish(RuntimeEventKind::DeviceConnection {
                device_ref: self.device_ref,
                phase: DeviceConnectionPhase::Connecting,
                retry_in_ms: None,
                message: None,
            });
            let password = match self.runtime.passwords.load_password(self.device_ref).await {
                Ok(password) => password,
                Err(error) => {
                    self.publish(RuntimeEventKind::DeviceConnection {
                        device_ref: self.device_ref,
                        phase: DeviceConnectionPhase::Disconnected,
                        retry_in_ms: None,
                        message: Some(error.to_string()),
                    });
                    return Err(error.into());
                }
            };
            let mode = self.connection_mode();
            match connector.connect_device_with_mode(self.device_ref, password, mode).await {
                Ok(connection) => {
                    if self.manually_disconnected.load(Ordering::Acquire) {
                        connection.close();
                        return Err(RuntimeError::DeviceDisconnected);
                    }
                    let mut cached = self.connection.write().await;
                    if self.connection_mode() != mode {
                        connection.close();
                        continue;
                    }
                    let connection = Arc::new(connection);
                    *cached = Some(Arc::clone(&connection));
                    drop(cached);
                    let scope = {
                        let presence = self
                            .runtime
                            .presence
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        presence
                            .account
                            .as_ref()
                            .and_then(|account| account.user.as_ref())
                            .and_then(|user| {
                                self.runtime
                                    .store
                                    .account_origin
                                    .as_ref()
                                    .map(|origin| format!("{origin}\n{}", user.user_id))
                            })
                    };
                    self.sample_usage(scope.as_deref()).await;
                    self.publish(RuntimeEventKind::DeviceConnection {
                        device_ref: self.device_ref,
                        phase: DeviceConnectionPhase::Connected,
                        retry_in_ms: None,
                        message: None,
                    });
                    return Ok(connection);
                }
                Err(error) if error.is_recoverable_connection() => {
                    tracing::warn!(device_id = %self.device_ref.device_id, %error, "device connection attempt failed, retrying");
                    self.publish(RuntimeEventKind::DeviceConnection {
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
                    self.publish(RuntimeEventKind::DeviceConnection {
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
            self.finish_usage(failed).await;
            tracing::warn!(device_id = %self.device_ref.device_id, %error,
                "replacing failed device connection; connection-bound sessions will end");
            connection.take();
            failed.as_ref().clone().close();
        }
        drop(connection);
        self.publish(RuntimeEventKind::DeviceConnection {
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

#[cfg(test)]
mod mode_tests {
    use super::*;

    #[tokio::test]
    async fn file_relay_profiles_never_replace_the_shared_desktop_connection() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime,_credentials) = crate::runtime::test_support::runtime(dir.path()).await;
        let (_a,_b,client,peer) = crate::connection::test_connection_pair().await;
        let reference = client.device_ref();
        assert!(runtime.file_profile_observation(reference,false).await.is_none());
        assert!(runtime.inner.file_devices.lock().await.is_empty(),"status must not open a file profile");
        let common = runtime.inner.device(reference).await;
        common.set_test_connection(client).await;
        assert!(runtime.file_profile_observation(reference,false).await.is_none(),"status must not borrow the desktop transport");
        let auto = runtime.inner.file_device(reference,false).await.unwrap();
        let relay = runtime.inner.file_device(reference,true).await.unwrap();
        assert!(!Arc::ptr_eq(&common,&auto));
        assert!(!Arc::ptr_eq(&auto,&relay));
        assert!(Arc::ptr_eq(&relay,&runtime.inner.file_device(reference,true).await.unwrap()));
        assert_ne!(common.usage_session,auto.usage_session);
        assert_ne!(auto.usage_session,relay.usage_session);
        assert_eq!(relay.connection_mode(),crate::ConnectionMode::Relay);
        assert_eq!(common.connection_mode(),crate::ConnectionMode::Auto);
        relay.close_for_account_change().await;
        assert!(common.cached_connection().await.is_some());
        assert!(common.cached_connection().await.unwrap().connection().selected_path().is_some());
        assert_eq!(runtime.inner.all_devices().await.len(),3);
        peer.close(b"test done");
        runtime.inner.file_devices.lock().await.clear();
        runtime.inner.devices.lock().await.clear();
        drop((common,auto,relay));
        Arc::try_unwrap(runtime).ok().unwrap().shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn changing_mode_closes_the_cached_connection_and_keeps_other_device_choices() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime,_credentials) = crate::runtime::test_support::runtime(dir.path()).await;
        let (_a, _b, client, peer) = crate::connection::test_connection_pair().await;
        let device_ref = client.device_ref();
        let device = runtime.inner.device(device_ref).await;
        let other = runtime.inner.device(DeviceRef { device_id: pab_protocol::DeviceId::new(), ..device_ref }).await;
        let transport = client.connection().clone();
        device.set_test_connection(client).await;
        device.set_connection_mode(crate::ConnectionMode::Auto).await;
        assert!(device.cached_connection().await.is_some(), "repeating the current choice must reuse the connection");
        device.set_connection_mode(crate::ConnectionMode::Relay).await;
        assert!(device.cached_connection().await.is_none());
        tokio::time::timeout(Duration::from_secs(5), transport.closed()).await.unwrap();
        assert_eq!(device.connection_mode(), crate::ConnectionMode::Relay);
        assert_eq!(other.connection_mode(), crate::ConnectionMode::Auto);
        // Reconnects and implicit operations preserve the session preference.
        device.resume();
        assert_eq!(device.connection_mode(), crate::ConnectionMode::Relay);
        device.set_connection_mode(crate::ConnectionMode::Auto).await;
        assert_eq!(device.connection_mode(), crate::ConnectionMode::Auto);
        peer.close(b"test done");
        Arc::try_unwrap(runtime).ok().unwrap().shutdown().await.unwrap();
    }
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
