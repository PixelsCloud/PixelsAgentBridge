use std::{
    path::Path,
    time::{Duration, SystemTime},
};

use pab_agent_core::{DevicePeerAuthorizer, PeerAuthorizationError};
use pab_protocol::{
    DEVICE_SESSION_AUTH_SCHEMA_VERSION, DeviceRef, DeviceSessionAuthenticate,
    DeviceSessionAuthenticationResult, EndpointKey, MAX_DEVICE_PASSWORD_BYTES,
};
use pab_transport::{PabConnection, PabConnectionError};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::credential::{DeviceCredential, DeviceCredentialError};
use crate::task_service::TaskService;

const FAILED_PASSWORD_DELAY: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub struct DeviceSessionAcceptor {
    authorizer: DevicePeerAuthorizer,
    credential: DeviceCredential,
    device_ref: DeviceRef,
    timeout: Duration,
    tasks: Option<TaskService>,
}

impl DeviceSessionAcceptor {
    pub fn from_credential_file(
        authorizer: DevicePeerAuthorizer,
        device_ref: DeviceRef,
        path: &Path,
        timeout: Duration,
    ) -> Result<Self, DeviceSessionError> {
        if timeout.is_zero() {
            return Err(DeviceSessionError::InvalidTimeout);
        }
        Ok(Self {
            authorizer,
            credential: DeviceCredential::read(path)?,
            device_ref,
            timeout,
            tasks: None,
        })
    }

    pub(crate) fn new(
        authorizer: DevicePeerAuthorizer,
        credential: DeviceCredential,
        device_ref: DeviceRef,
        timeout: Duration,
    ) -> Self {
        Self {
            authorizer,
            credential,
            device_ref,
            timeout,
            tasks: None,
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
                .map_err(|error| DeviceSessionError::TaskService(error.to_string()))?,
        );
        Ok(self)
    }

    pub(crate) fn with_task_service(mut self, tasks: TaskService) -> Self {
        self.tasks = Some(tasks);
        self
    }

    pub async fn handle(&self, connection: PabConnection) -> Result<(), DeviceSessionError> {
        authenticate(
            connection,
            self.authorizer.clone(),
            self.credential.clone(),
            self.device_ref,
            self.timeout,
            self.tasks.clone(),
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
        Ok(authorized) if authorized.device_ref == device_ref => authorized,
        Ok(_) => {
            reject(&mut stream, timeout).await?;
            return Err(DeviceSessionError::IdentityMismatch);
        }
        Err(error) => {
            reject(&mut stream, timeout).await?;
            return Err(DeviceSessionError::PeerAuthorization(error));
        }
    };

    if !credential.verify(password).await? {
        tokio::time::sleep(FAILED_PASSWORD_DELAY).await;
        reject(&mut stream, timeout).await?;
        return Err(DeviceSessionError::Rejected);
    }

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
        loop {
            match connection.accept_bi(timeout).await {
                Ok(stream) => {
                    let tasks = tasks.clone();
                    let initiated_by = authorized.operator;
                    tokio::spawn(async move {
                        if let Err(error) = tasks.handle_stream(initiated_by, stream, timeout).await
                            && !error.is_connection_end()
                        {
                            eprintln!("pab-executor: task_stream={error}");
                        }
                    });
                }
                Err(PabConnectionError::Timeout) => continue,
                Err(_) => break,
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
