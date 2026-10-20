use std::{path::Path, sync::Arc, time::Duration};

use pab_agent_core::{DevicePeerAuthorizer, PeerAuthorizationError};
use pab_device_session::{PasswordAttempts, SessionError};
use pab_protocol::DeviceRef;
use pab_transport::{PabConnection, PabConnectionError};
use thiserror::Error;
use tokio::time::MissedTickBehavior;

use crate::credential::{DeviceCredential, DeviceCredentialError};
use crate::task_service::TaskService;

const AUTHORIZATION_RECHECK_INTERVAL: Duration = Duration::from_secs(5);
type FailedPasswords = PasswordAttempts;

pub(crate) use pab_device_session::ActiveSessions;

#[derive(Clone)]
pub struct DeviceSessionAcceptor {
    authorizer: DevicePeerAuthorizer,
    credential_path: std::path::PathBuf,
    device_ref: DeviceRef,
    timeout: Duration,
    tasks: Option<TaskService>,
    failed_passwords: FailedPasswords,
    active_sessions: ActiveSessions,
}

impl DeviceSessionAcceptor {
    pub async fn from_credential_file(
        authorizer: DevicePeerAuthorizer,
        device_ref: DeviceRef,
        path: &Path,
        timeout: Duration,
    ) -> Result<Self, DeviceSessionError> {
        if timeout.is_zero() {
            return Err(DeviceSessionError::InvalidTimeout);
        }
        DeviceCredential::read(path).await?;
        Ok(Self {
            authorizer,
            credential_path: path.to_path_buf(),
            device_ref,
            timeout,
            tasks: None,
            failed_passwords: PasswordAttempts::default(),
            active_sessions: ActiveSessions::default(),
        })
    }

    pub(crate) fn new(
        authorizer: DevicePeerAuthorizer,
        credential_path: std::path::PathBuf,
        device_ref: DeviceRef,
        timeout: Duration,
    ) -> Self {
        Self {
            authorizer,
            credential_path,
            device_ref,
            timeout,
            tasks: None,
            failed_passwords: PasswordAttempts::default(),
            active_sessions: ActiveSessions::default(),
        }
    }

    pub async fn with_task_database(
        mut self,
        database_file: &Path,
        execution_context: pab_protocol::ExecutionContext,
    ) -> Result<Self, DeviceSessionError> {
        self.tasks = Some(
            TaskService::open(database_file, self.device_ref, execution_context)
                .await
                .map_err(|error| DeviceSessionError::TaskService(error.to_string()))?
                .with_active_sessions(self.active_sessions.clone()),
        );
        Ok(self)
    }

    pub(crate) fn with_task_service(mut self, tasks: TaskService) -> Self {
        self.tasks = Some(tasks.with_active_sessions(self.active_sessions.clone()));
        self
    }

    /// Register native capture only after its platform helper is available.
    /// This adds media capability to the existing authenticated session handler.
    pub fn with_remote_desktop_backend(
        mut self,
        backend: Arc<dyn pab_remote_desktop::DesktopBackend>,
    ) -> Result<Self, DeviceSessionError> {
        let tasks = self.tasks.take().ok_or_else(|| {
            DeviceSessionError::TaskService(
                "configure the task service before the remote desktop backend".into(),
            )
        })?;
        self.tasks = Some(tasks.with_remote_desktop_backend(backend));
        Ok(self)
    }

    pub async fn handle(&self, connection: PabConnection) -> Result<(), DeviceSessionError> {
        let credential = DeviceCredential::read(&self.credential_path).await?;
        authenticate(
            connection,
            self.authorizer.clone(),
            credential,
            self.device_ref,
            self.timeout,
            self.tasks.clone(),
            self.failed_passwords.clone(),
            self.active_sessions.clone(),
        )
        .await
    }
}

async fn authenticate(
    connection: PabConnection,
    authorizer: DevicePeerAuthorizer,
    credential: DeviceCredential,
    device_ref: DeviceRef,
    timeout: Duration,
    tasks: Option<TaskService>,
    failed_passwords: FailedPasswords,
    active_sessions: ActiveSessions,
) -> Result<(), DeviceSessionError> {
    let password = credential.shared().map_err(map_session_error)?;
    let authorized = pab_device_session::authenticate(
        &connection,
        &authorizer,
        &password,
        device_ref,
        timeout,
        &failed_passwords,
    )
    .await
    .map_err(map_session_error)?;
    let peer_endpoint_key = authorized.peer_endpoint_key;
    let _session_lease = active_sessions.enter(peer_endpoint_key);
    if let Some(tasks) = tasks {
        let tasks = tasks.for_ui_connection();
        let _terminals = tasks.terminal_connection_guard();
        let _desktop_terminals = tasks.desktop_terminal_connection_guard();
        let _remote_desktop = tasks.remote_desktop_connection_guard();
        let mut recheck = tokio::time::interval(AUTHORIZATION_RECHECK_INTERVAL);
        recheck.set_missed_tick_behavior(MissedTickBehavior::Delay);
        recheck.tick().await;
        loop {
            tokio::select! {
                _ = recheck.tick() => {
                    let current_authorization = authorizer.report_authenticated(peer_endpoint_key).await;
                    if let Ok(current) = &current_authorization {
                        tasks.recheck_desktop_terminal_account(current.user_context.user.as_ref().map(|u| u.user_id));
                        tasks.recheck_remote_desktop_account(current.user_context.user.as_ref().map(|u| u.user_id));
                    }
                    let still_authorized = matches!(
                        current_authorization,
                        Ok(current)
                            if current.device_ref == device_ref
                                && current.peer_endpoint_key == peer_endpoint_key
                                && current.operator == authorized.operator
                    );
                    if !still_authorized {
                        connection.close(b"device authorization expired");
                        return Err(DeviceSessionError::AuthorizationExpired);
                    }
                }
                incoming = connection.accept_bi(timeout) => match incoming {
                    Ok(stream) => {
                        let tasks = tasks.clone();
                        let initiated_by = authorized.operator;
                        let current = match authorizer.authorize(peer_endpoint_key).await {
                            Ok(current) if current.operator == initiated_by && current.device_ref == device_ref => current,
                            _ => {
                                connection.close(b"device authorization expired");
                                return Err(DeviceSessionError::AuthorizationExpired);
                            }
                        };
                        let tasks = tasks.with_user_context(current.user_context);
                        tokio::spawn(async move {
                            // Resolve the current user at the start of each stream. Already
                            // running streams keep their original immutable attribution.
                            if let Err(error) = tasks.handle_stream(initiated_by, stream, timeout).await
                                && !error.is_connection_end()
                            {
                                tracing::warn!(%error, "task stream failed");
                            }
                        });
                    }
                    Err(PabConnectionError::Timeout) => continue,
                    Err(error) => {
                        tracing::info!(%error, "authenticated device connection ended");
                        break;
                    }
                },
            }
        }
    } else {
        connection.closed().await;
    }
    Ok(())
}

fn map_session_error(error: SessionError) -> DeviceSessionError {
    match error {
        SessionError::Rejected => DeviceSessionError::Rejected,
        SessionError::IdentityMismatch => DeviceSessionError::IdentityMismatch,
        SessionError::InvalidTimeout => DeviceSessionError::InvalidTimeout,
        SessionError::InvalidTime => DeviceSessionError::InvalidSystemTime,
        SessionError::Authorization(error) => DeviceSessionError::PeerAuthorization(error),
        SessionError::Connection(error) => DeviceSessionError::Connection(error),
        error => DeviceSessionError::TaskService(error.to_string()),
    }
}

#[derive(Debug, Error)]
pub enum DeviceSessionError {
    #[error("device session authentication was rejected")]
    Rejected,
    #[error("authorized peer identity does not match this device session")]
    IdentityMismatch,
    #[error("device peer authorization expired or could not be refreshed")]
    AuthorizationExpired,
    #[error("system time is outside the supported Unix timestamp range")]
    InvalidSystemTime,
    #[error("device session timeout must be greater than zero")]
    InvalidTimeout,
    #[error(transparent)]
    PeerAuthorization(#[from] PeerAuthorizationError),
    #[error(transparent)]
    Credential(#[from] DeviceCredentialError),
    #[error(transparent)]
    Connection(#[from] PabConnectionError),
    #[error("the local task service failed to start: {0}")]
    TaskService(String),
}

#[cfg(test)]
mod presence_tests {
    use super::*;
    use pab_protocol::EndpointKey;

    #[test]
    fn active_operator_count_tracks_distinct_endpoint_keys() {
        let sessions = ActiveSessions::default();
        let first_key = EndpointKey::new([1; 32]);
        let second_key = EndpointKey::new([2; 32]);
        let first = sessions.enter(first_key);
        let duplicate = sessions.enter(first_key);
        assert_eq!(sessions.operator_count(), 1);
        let second = sessions.enter(second_key);
        assert_eq!(sessions.operator_count(), 2);
        drop(first);
        assert_eq!(sessions.operator_count(), 2);
        drop(duplicate);
        assert_eq!(sessions.operator_count(), 1);
        drop(second);
        assert_eq!(sessions.operator_count(), 0);
    }
}
