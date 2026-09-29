use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant, SystemTime},
};

use pab_agent_core::{DevicePeerAuthorizer, PeerAuthorizationError};
use pab_protocol::{
    DEVICE_SESSION_AUTH_SCHEMA_VERSION, DeviceRef, DeviceSessionAuthenticate,
    DeviceSessionAuthenticationResult, EndpointKey, MAX_DEVICE_PASSWORD_BYTES,
};
use pab_transport::{PabConnection, PabConnectionError};
use thiserror::Error;
use tokio::sync::Mutex;
use tokio::time::MissedTickBehavior;
use zeroize::Zeroizing;

use crate::credential::{DeviceCredential, DeviceCredentialError};
use crate::task_service::TaskService;

const FAILED_PASSWORD_DELAY: Duration = Duration::from_millis(500);
const PASSWORD_ATTEMPT_WINDOW: Duration = Duration::from_secs(600);
const MAX_PASSWORD_FAILURES: usize = 5;
const AUTHORIZATION_RECHECK_INTERVAL: Duration = Duration::from_secs(5);
type FailedPasswords = Arc<Mutex<HashMap<EndpointKey, Vec<Instant>>>>;

#[derive(Clone, Default)]
pub(crate) struct ActiveSessions(Arc<StdMutex<HashMap<EndpointKey, u16>>>);

impl ActiveSessions {
    fn enter(&self, endpoint_key: EndpointKey) -> ActiveSessionLease {
        let mut sessions = self.0.lock().unwrap();
        *sessions.entry(endpoint_key).or_default() += 1;
        ActiveSessionLease {
            sessions: self.clone(),
            endpoint_key,
        }
    }

    pub(crate) fn operator_count(&self) -> u16 {
        self.0.lock().unwrap().len() as u16
    }
}

struct ActiveSessionLease {
    sessions: ActiveSessions,
    endpoint_key: EndpointKey,
}

impl Drop for ActiveSessionLease {
    fn drop(&mut self) {
        let mut sessions = self.sessions.0.lock().unwrap();
        if let Some(count) = sessions.get_mut(&self.endpoint_key) {
            *count -= 1;
            if *count == 0 {
                sessions.remove(&self.endpoint_key);
            }
        }
    }
}

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
            failed_passwords: Arc::default(),
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
            failed_passwords: Arc::default(),
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

    pub async fn handle(&self, connection: PabConnection) -> Result<(), DeviceSessionError> {
        let credential = DeviceCredential::read(&self.credential_path).await?;
        authenticate(
            connection,
            self.authorizer.clone(),
            credential,
            self.device_ref,
            self.timeout,
            self.tasks.clone(),
            Arc::clone(&self.failed_passwords),
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
    let mut stream = connection.accept_bi(timeout).await?;
    let request: DeviceSessionAuthenticate = stream.receive_sensitive_json(timeout).await?;
    let password = Zeroizing::new(request.device_password);
    if request.schema_version != DEVICE_SESSION_AUTH_SCHEMA_VERSION
        || request.device_ref != device_ref
        || password.is_empty()
        || password.len() > MAX_DEVICE_PASSWORD_BYTES
    {
        reject(&mut stream, timeout).await?;
        return Err(DeviceSessionError::Rejected);
    }

    let peer_endpoint_key = EndpointKey::new(connection.remote_endpoint_key());
    let authorized = match authorizer.authorize(peer_endpoint_key).await {
        Ok(authorized)
            if authorized.device_ref == device_ref
                && authorized.peer_endpoint_key == peer_endpoint_key
                && authorized.operator.endpoint_key() == peer_endpoint_key =>
        {
            authorized
        }
        Ok(_) => {
            reject(&mut stream, timeout).await?;
            return Err(DeviceSessionError::IdentityMismatch);
        }
        Err(error) => {
            reject(&mut stream, timeout).await?;
            return Err(DeviceSessionError::PeerAuthorization(error));
        }
    };

    let locked_out = {
        let mut failures = failed_passwords.lock().await;
        let now = Instant::now();
        failures.retain(|_, attempts| {
            attempts.retain(|at| now.duration_since(*at) < PASSWORD_ATTEMPT_WINDOW);
            !attempts.is_empty()
        });
        failures
            .get(&peer_endpoint_key)
            .is_some_and(|attempts| attempts.len() >= MAX_PASSWORD_FAILURES)
    };
    if locked_out {
        reject(&mut stream, timeout).await?;
        return Err(DeviceSessionError::Rejected);
    }

    if !credential.verify(password).await? {
        failed_passwords
            .lock()
            .await
            .entry(peer_endpoint_key)
            .or_default()
            .push(Instant::now());
        tokio::time::sleep(FAILED_PASSWORD_DELAY).await;
        reject(&mut stream, timeout).await?;
        return Err(DeviceSessionError::Rejected);
    }
    failed_passwords.lock().await.remove(&peer_endpoint_key);

    let _session_lease = active_sessions.enter(peer_endpoint_key);

    stream
        .send_json(
            &DeviceSessionAuthenticationResult::Accepted {
                device_ref,
                operator: authorized.operator,
                password_version: credential.password_version(),
                authenticated_at_unix_ms: unix_millis(SystemTime::now())?,
            },
            timeout,
        )
        .await?;
    if let Some(tasks) = tasks {
        let mut recheck = tokio::time::interval(AUTHORIZATION_RECHECK_INTERVAL);
        recheck.set_missed_tick_behavior(MissedTickBehavior::Delay);
        recheck.tick().await;
        loop {
            tokio::select! {
                _ = recheck.tick() => {
                    let still_authorized = matches!(
                        authorizer.authorize(peer_endpoint_key).await,
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
                        tokio::spawn(async move {
                            if let Err(error) = tasks.handle_stream(initiated_by, stream, timeout).await
                                && !error.is_connection_end()
                            {
                                tracing::warn!(%error, "task stream failed");
                            }
                        });
                    }
                    Err(PabConnectionError::Timeout) => continue,
                    Err(_) => break,
                },
            }
        }
    } else {
        connection.closed().await;
    }
    Ok(())
}

async fn reject(
    stream: &mut pab_transport::PabBiStream,
    timeout: Duration,
) -> Result<(), PabConnectionError> {
    stream
        .send_json(&DeviceSessionAuthenticationResult::Rejected, timeout)
        .await
}

fn unix_millis(now: SystemTime) -> Result<i64, DeviceSessionError> {
    let value = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| DeviceSessionError::InvalidSystemTime)?
        .as_millis();
    i64::try_from(value).map_err(|_| DeviceSessionError::InvalidSystemTime)
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
