use std::fs;

use pab_agent_core::{
    ControlConnectionStatus, DeviceNetworkResolutionError, DeviceNetworkResolver,
    EndpointControlConfig, EndpointControlSupervisor, EndpointControlSupervisorHandle,
    EndpointSecretError, ReconnectPolicy, ReconnectPolicyError, TlsConnectorError,
    read_endpoint_secret, tls_connector,
};
use pab_protocol::{
    CommandTaskSpec, ContextFreshness, DEVICE_SESSION_AUTH_SCHEMA_VERSION,
    DEVICE_TASK_SCHEMA_VERSION, DeviceRef, DeviceSessionAuthenticate,
    DeviceSessionAuthenticationResult, DeviceTaskErrorCode, DeviceTaskRequest, DeviceTaskResponse,
    EndpointProofPrincipal, MAX_DEVICE_PASSWORD_BYTES, MAX_OUTPUT_READ_BYTES, OutputStream,
    RequestId, TASK_SCHEMA_VERSION, TargetContext, TargetContextSource, TaskRef, TaskSnapshot,
    UserId,
};
use pab_transport::{
    PabConnection, PabConnectionError, PabEndpoint, PabEndpointAddress, PabEndpointConfig,
    PabEndpointError,
};
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

use crate::{BridgeConfig, BridgeConfigError};

pub struct BridgeClient {
    connector: BridgeConnector,
    control: EndpointControlSupervisorHandle,
}

#[derive(Clone)]
pub struct BridgeConnector {
    config: BridgeConfig,
    network_resolver: DeviceNetworkResolver,
    endpoint: PabEndpoint,
}

impl BridgeClient {
    pub async fn connect(config: BridgeConfig) -> Result<Self, BridgeError> {
        config.validate()?;
        let secret = read_endpoint_secret(&config.endpoint_secret_file)?;
        let control_ca = config
            .control_ca_cert
            .as_ref()
            .map(|path| read_file(path, "control CA"))
            .transpose()?;
        let connector = tls_connector(control_ca.as_deref())?;
        let control = EndpointControlSupervisor::new(
            EndpointControlConfig {
                url: config.control_url.clone(),
                deployment_id: config.deployment_id,
                tenant_id: config.tenant_id,
                principal: EndpointProofPrincipal::User {
                    user_id: config.user_id,
                },
                operation_timeout: config.operation_timeout,
            },
            secret.clone(),
            connector,
            ReconnectPolicy::default(),
        )?
        .spawn();
        let network_resolver = control.device_network_resolver();

        let mut endpoint_config = PabEndpointConfig::new(config.relay_urls.clone())?;
        if let Some(path) = &config.relay_ca_cert {
            endpoint_config = endpoint_config.with_extra_ca_pem(&read_file(path, "Relay CA")?)?;
        }
        let endpoint = PabEndpoint::bind(endpoint_config, secret).await?;
        endpoint.wait_online(config.operation_timeout).await?;
        Ok(Self {
            connector: BridgeConnector {
                config,
                network_resolver,
                endpoint,
            },
            control,
        })
    }

    pub fn control_status(&self) -> tokio::sync::watch::Receiver<ControlConnectionStatus> {
        self.control.status()
    }

    pub fn connector(&self) -> BridgeConnector {
        self.connector.clone()
    }

    pub async fn connect_device(
        &self,
        device_ref: DeviceRef,
        password: Zeroizing<String>,
    ) -> Result<AuthenticatedDeviceConnection, BridgeError> {
        self.connector.connect_device(device_ref, password).await
    }

    pub async fn shutdown(self) -> Result<(), BridgeError> {
        self.connector.endpoint.close().await;
        self.control.shutdown().await?;
        Ok(())
    }
}

impl BridgeConnector {
    pub async fn resolve_device_code(
        &self,
        code: pab_protocol::DeviceCode,
    ) -> Result<DeviceRef, BridgeError> {
        let device_ref = self.network_resolver.resolve_code(code).await?;
        if device_ref.deployment_id != self.config.deployment_id
            || device_ref.tenant_id != self.config.tenant_id
        {
            return Err(BridgeError::DeviceIdentityMismatch);
        }
        Ok(device_ref)
    }

    pub async fn connect_device(
        &self,
        device_ref: DeviceRef,
        password: Zeroizing<String>,
    ) -> Result<AuthenticatedDeviceConnection, BridgeError> {
        if device_ref.deployment_id != self.config.deployment_id
            || device_ref.tenant_id != self.config.tenant_id
        {
            return Err(BridgeError::DeviceIdentityMismatch);
        }
        if password.is_empty() || password.len() > MAX_DEVICE_PASSWORD_BYTES {
            return Err(BridgeError::InvalidDevicePassword);
        }
        let snapshot = self.network_resolver.resolve(device_ref).await?;
        let address = PabEndpointAddress {
            relay_urls: snapshot.relay_urls,
            direct_addresses: snapshot.direct_addresses,
        };
        let connection = self
            .endpoint
            .connect(
                *snapshot.endpoint_key.as_bytes(),
                &address,
                self.config.operation_timeout,
            )
            .await?;
        let mut stream = connection.open_bi(self.config.operation_timeout).await?;
        let mut request = DeviceSessionAuthenticate {
            schema_version: DEVICE_SESSION_AUTH_SCHEMA_VERSION,
            device_ref,
            device_password: password.to_string(),
        };
        let send_result = stream
            .send_sensitive_json(&request, self.config.operation_timeout)
            .await;
        request.device_password.zeroize();
        send_result?;
        let result: DeviceSessionAuthenticationResult =
            stream.receive_json(self.config.operation_timeout).await?;
        match result {
            DeviceSessionAuthenticationResult::Accepted {
                device_ref: accepted_device,
                peer_user_id,
                password_version,
                authenticated_at_unix_ms,
            } if accepted_device == device_ref
                && peer_user_id == self.config.user_id
                && password_version > 0
                && authenticated_at_unix_ms > 0 =>
            {
                Ok(AuthenticatedDeviceConnection {
                    connection,
                    device_ref,
                    peer_user_id,
                    password_version,
                    authenticated_at_unix_ms,
                    operation_timeout: self.config.operation_timeout,
                })
            }
            DeviceSessionAuthenticationResult::Rejected => {
                connection.close(b"device authentication rejected");
                Err(BridgeError::AuthenticationRejected)
            }
            DeviceSessionAuthenticationResult::Accepted { .. } => {
                connection.close(b"device authentication identity mismatch");
                Err(BridgeError::AuthenticationIdentityMismatch)
            }
        }
    }
}

#[derive(Clone)]
pub struct AuthenticatedDeviceConnection {
    connection: PabConnection,
    device_ref: DeviceRef,
    peer_user_id: UserId,
    password_version: u64,
    authenticated_at_unix_ms: i64,
    operation_timeout: std::time::Duration,
}

impl AuthenticatedDeviceConnection {
    pub const fn device_ref(&self) -> DeviceRef {
        self.device_ref
    }

    pub const fn peer_user_id(&self) -> UserId {
        self.peer_user_id
    }

    pub const fn password_version(&self) -> u64 {
        self.password_version
    }

    pub const fn authenticated_at_unix_ms(&self) -> i64 {
        self.authenticated_at_unix_ms
    }

    pub fn connection(&self) -> &PabConnection {
        &self.connection
    }

    pub async fn get_environment(&self) -> Result<TargetContext, BridgeError> {
        match self
            .task_request(DeviceTaskRequest::GetEnvironment {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
            })
            .await?
        {
            DeviceTaskResponse::Environment { context }
                if context.device_ref == self.device_ref
                    && context.source == TargetContextSource::ExecutorVerified
                    && context.freshness == ContextFreshness::Current =>
            {
                Ok(*context)
            }
            response => Err(unexpected_task_response(response)),
        }
    }

    pub async fn submit_command(
        &self,
        request_id: RequestId,
        command: CommandTaskSpec,
    ) -> Result<TaskSnapshot, BridgeError> {
        match self
            .task_request(DeviceTaskRequest::SubmitCommand {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id,
                command,
            })
            .await?
        {
            DeviceTaskResponse::Submitted { snapshot }
                if valid_snapshot(
                    &snapshot,
                    self.device_ref,
                    self.peer_user_id,
                    Some(request_id),
                    None,
                ) =>
            {
                Ok(*snapshot)
            }
            response => Err(unexpected_task_response(response)),
        }
    }

    pub async fn get_task(&self, task_ref: TaskRef) -> Result<TaskSnapshot, BridgeError> {
        self.validate_task_ref(task_ref)?;
        match self
            .task_request(DeviceTaskRequest::GetTask {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                task_ref,
            })
            .await?
        {
            DeviceTaskResponse::Snapshot { snapshot }
                if valid_snapshot(
                    &snapshot,
                    self.device_ref,
                    self.peer_user_id,
                    None,
                    Some(task_ref),
                ) =>
            {
                Ok(*snapshot)
            }
            response => Err(unexpected_task_response(response)),
        }
    }

    pub async fn read_output(
        &self,
        task_ref: TaskRef,
        stream: OutputStream,
        offset: u64,
        max_bytes: u32,
    ) -> Result<(pab_protocol::OutputChunk, pab_protocol::OutputRange), BridgeError> {
        self.validate_task_ref(task_ref)?;
        if max_bytes == 0 || max_bytes > MAX_OUTPUT_READ_BYTES {
            return Err(BridgeError::InvalidOutputReadSize);
        }
        match self
            .task_request(DeviceTaskRequest::ReadOutput {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                task_ref,
                stream,
                offset,
                max_bytes,
            })
            .await?
        {
            DeviceTaskResponse::Output { chunk, range }
                if chunk.schema_version == TASK_SCHEMA_VERSION
                    && chunk.task_ref == task_ref
                    && chunk.stream == stream
                    && chunk.offset == offset
                    && range.retained_from <= chunk.offset
                    && chunk
                        .offset
                        .checked_add(u64::try_from(chunk.bytes.len()).unwrap_or(u64::MAX))
                        .is_some_and(|end| end <= range.available_to) =>
            {
                Ok((chunk, range))
            }
            response => Err(unexpected_task_response(response)),
        }
    }

    pub async fn cancel_task(
        &self,
        task_ref: TaskRef,
        reason: String,
    ) -> Result<TaskSnapshot, BridgeError> {
        self.validate_task_ref(task_ref)?;
        match self
            .task_request(DeviceTaskRequest::Cancel {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                task_ref,
                reason,
            })
            .await?
        {
            DeviceTaskResponse::CancelAccepted { snapshot }
                if valid_snapshot(
                    &snapshot,
                    self.device_ref,
                    self.peer_user_id,
                    None,
                    Some(task_ref),
                ) =>
            {
                Ok(*snapshot)
            }
            response => Err(unexpected_task_response(response)),
        }
    }

    pub async fn subscribe_task(
        &self,
        task_ref: TaskRef,
        after_event_seq: u64,
        stdout_offset: u64,
        stderr_offset: u64,
    ) -> Result<TaskSubscription, BridgeError> {
        self.validate_task_ref(task_ref)?;
        let mut stream = self.connection.open_bi(self.operation_timeout()).await?;
        stream
            .send_json(
                &DeviceTaskRequest::Subscribe {
                    schema_version: DEVICE_TASK_SCHEMA_VERSION,
                    task_ref,
                    after_event_seq,
                    stdout_offset,
                    stderr_offset,
                },
                self.operation_timeout(),
            )
            .await?;
        Ok(TaskSubscription {
            stream,
            task_ref,
            initiated_by: self.peer_user_id,
        })
    }

    async fn task_request(
        &self,
        request: DeviceTaskRequest,
    ) -> Result<DeviceTaskResponse, BridgeError> {
        let mut stream = self.connection.open_bi(self.operation_timeout()).await?;
        stream.send_json(&request, self.operation_timeout()).await?;
        let response: DeviceTaskResponse = stream.receive_json(self.operation_timeout()).await?;
        match response {
            DeviceTaskResponse::Error { code, message } => {
                Err(BridgeError::RemoteTask { code, message })
            }
            response => Ok(response),
        }
    }

    fn operation_timeout(&self) -> std::time::Duration {
        self.operation_timeout
    }

    fn validate_task_ref(&self, task_ref: TaskRef) -> Result<(), BridgeError> {
        if task_ref.device_ref != self.device_ref {
            return Err(BridgeError::TaskDeviceMismatch);
        }
        Ok(())
    }

    pub fn close(self) {
        self.connection.close(b"Bridge device session closed");
    }
}

pub struct TaskSubscription {
    stream: pab_transport::PabBiStream,
    task_ref: TaskRef,
    initiated_by: UserId,
}

impl TaskSubscription {
    pub async fn next(&mut self) -> Result<DeviceTaskResponse, BridgeError> {
        let response: DeviceTaskResponse = self.stream.receive_json_wait().await?;
        match &response {
            DeviceTaskResponse::Error { code, message } => {
                return Err(BridgeError::RemoteTask {
                    code: *code,
                    message: message.clone(),
                });
            }
            DeviceTaskResponse::Event { event }
                if event.schema_version != TASK_SCHEMA_VERSION
                    || event.task_ref != self.task_ref =>
            {
                return Err(unexpected_task_response(response));
            }
            DeviceTaskResponse::Output { chunk, range }
                if chunk.schema_version != TASK_SCHEMA_VERSION
                    || chunk.task_ref != self.task_ref
                    || range.retained_from > chunk.offset
                    || chunk
                        .offset
                        .checked_add(u64::try_from(chunk.bytes.len()).unwrap_or(u64::MAX))
                        .is_none_or(|end| end > range.available_to) =>
            {
                return Err(unexpected_task_response(response));
            }
            DeviceTaskResponse::OutputChanged { task_ref, .. } if *task_ref != self.task_ref => {
                return Err(unexpected_task_response(response));
            }
            DeviceTaskResponse::CaughtUp { snapshot }
                if !valid_snapshot(
                    snapshot,
                    self.task_ref.device_ref,
                    self.initiated_by,
                    None,
                    Some(self.task_ref),
                ) =>
            {
                return Err(unexpected_task_response(response));
            }
            _ => {}
        }
        Ok(response)
    }
}

fn valid_snapshot(
    snapshot: &TaskSnapshot,
    device_ref: DeviceRef,
    initiated_by: UserId,
    request_id: Option<RequestId>,
    task_ref: Option<TaskRef>,
) -> bool {
    snapshot.schema_version == TASK_SCHEMA_VERSION
        && snapshot.task_ref.device_ref == device_ref
        && snapshot.initiated_by == initiated_by
        && request_id.is_none_or(|expected| snapshot.request_id == expected)
        && task_ref.is_none_or(|expected| snapshot.task_ref == expected)
}

fn unexpected_task_response(response: DeviceTaskResponse) -> BridgeError {
    BridgeError::UnexpectedTaskResponse(format!("{response:?}"))
}

fn read_file(path: &std::path::Path, kind: &'static str) -> Result<Vec<u8>, BridgeError> {
    fs::read(path).map_err(|source| BridgeError::File {
        kind,
        path: path.to_owned(),
        source,
    })
}

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error(transparent)]
    Config(#[from] BridgeConfigError),
    #[error(transparent)]
    EndpointSecret(#[from] EndpointSecretError),
    #[error("the {kind} file {path} could not be read: {source}")]
    File {
        kind: &'static str,
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Tls(#[from] TlsConnectorError),
    #[error(transparent)]
    ReconnectPolicy(#[from] ReconnectPolicyError),
    #[error(transparent)]
    NetworkResolution(#[from] DeviceNetworkResolutionError),
    #[error(transparent)]
    Endpoint(#[from] PabEndpointError),
    #[error(transparent)]
    Connection(#[from] PabConnectionError),
    #[error("the endpoint control supervisor task failed: {0}")]
    SupervisorTask(#[from] tokio::task::JoinError),
    #[error("the requested device does not belong to this Bridge deployment and tenant")]
    DeviceIdentityMismatch,
    #[error("the device password must contain between 1 and {MAX_DEVICE_PASSWORD_BYTES} bytes")]
    InvalidDevicePassword,
    #[error("the device rejected authentication")]
    AuthenticationRejected,
    #[error("the device authentication response does not match the requested identity")]
    AuthenticationIdentityMismatch,
    #[error("the task belongs to a different device")]
    TaskDeviceMismatch,
    #[error("output read size is outside the supported range")]
    InvalidOutputReadSize,
    #[error("Executor task request failed with {code:?}: {message}")]
    RemoteTask {
        code: DeviceTaskErrorCode,
        message: String,
    },
    #[error("Executor returned an unexpected task response: {0}")]
    UnexpectedTaskResponse(String),
}

impl BridgeError {
    pub const fn is_recoverable_connection(&self) -> bool {
        matches!(
            self,
            Self::NetworkResolution(_) | Self::Endpoint(_) | Self::Connection(_)
        )
    }
}
