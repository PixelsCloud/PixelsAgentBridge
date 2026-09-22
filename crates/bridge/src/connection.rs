use std::fs;

use pab_agent_core::{
    AuthenticatedControlConnection, EndpointControlConfig, EndpointControlError,
    EndpointSecretError, TlsConnectorError, read_endpoint_secret, tls_connector,
};
use pab_protocol::{
    DEVICE_SESSION_AUTH_SCHEMA_VERSION, DeviceRef, DeviceSessionAuthenticate,
    DeviceSessionAuthenticationResult, EndpointProofPrincipal, MAX_DEVICE_PASSWORD_BYTES, UserId,
};
use pab_transport::{
    PabConnection, PabConnectionError, PabEndpoint, PabEndpointAddress, PabEndpointConfig,
    PabEndpointError,
};
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

use crate::{BridgeConfig, BridgeConfigError};

pub struct BridgeClient {
    config: BridgeConfig,
    control: AuthenticatedControlConnection,
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
        let control = AuthenticatedControlConnection::connect(
            &EndpointControlConfig {
                url: config.control_url.clone(),
                deployment_id: config.deployment_id,
                tenant_id: config.tenant_id,
                principal: EndpointProofPrincipal::User {
                    user_id: config.user_id,
                },
                operation_timeout: config.operation_timeout,
            },
            &secret,
            connector,
        )
        .await?;

        let mut endpoint_config = PabEndpointConfig::new(config.relay_urls.clone())?;
        if let Some(path) = &config.relay_ca_cert {
            endpoint_config = endpoint_config.with_extra_ca_pem(&read_file(path, "Relay CA")?)?;
        }
        let endpoint = PabEndpoint::bind(endpoint_config, secret).await?;
        endpoint.wait_online(config.operation_timeout).await?;
        Ok(Self {
            config,
            control,
            endpoint,
        })
    }

    pub async fn connect_device(
        &mut self,
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
        let snapshot = self
            .control
            .get_device_network(device_ref, self.config.operation_timeout)
            .await?;
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

    pub async fn shutdown(self) -> Result<(), BridgeError> {
        self.endpoint.close().await;
        self.control.close().await?;
        Ok(())
    }
}

pub struct AuthenticatedDeviceConnection {
    connection: PabConnection,
    device_ref: DeviceRef,
    peer_user_id: UserId,
    password_version: u64,
    authenticated_at_unix_ms: i64,
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

    pub fn close(self) {
        self.connection.close(b"Bridge device session closed");
    }
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
    Control(#[from] EndpointControlError),
    #[error(transparent)]
    Endpoint(#[from] PabEndpointError),
    #[error(transparent)]
    Connection(#[from] PabConnectionError),
    #[error("the requested device does not belong to this Bridge deployment and tenant")]
    DeviceIdentityMismatch,
    #[error("the device password must contain between 1 and {MAX_DEVICE_PASSWORD_BYTES} bytes")]
    InvalidDevicePassword,
    #[error("the device rejected authentication")]
    AuthenticationRejected,
    #[error("the device authentication response does not match the requested identity")]
    AuthenticationIdentityMismatch,
}
