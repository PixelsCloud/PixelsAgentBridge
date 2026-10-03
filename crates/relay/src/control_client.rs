use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use pab_protocol::{
    DeploymentId, RelayControlClientMessage, RelayControlErrorCode, RelayControlServerMessage,
    RequestId,
};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
    tungstenite::{Message, client::IntoClientRequest, http::header::AUTHORIZATION},
};

use crate::{PolicyRuntimeError, RelayPolicyRuntime};

const CONTROL_OPERATION_TIMEOUT: Duration = Duration::from_secs(10);

pub struct RelayControlClient {
    deployment_id: DeploymentId,
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl RelayControlClient {
    pub async fn connect(
        url: &str,
        deployment_id: DeploymentId,
        secret: &str,
        connector: Connector,
    ) -> Result<Self, RelayControlClientError> {
        let mut request = url.into_client_request()?;
        request.headers_mut().insert(
            AUTHORIZATION,
            format!("Bearer {secret}")
                .parse()
                .map_err(|_| RelayControlClientError::InvalidSecret)?,
        );
        let (socket, _) = tokio::time::timeout(
            CONTROL_OPERATION_TIMEOUT,
            connect_async_tls_with_config(request, None, false, Some(connector)),
        )
        .await
        .map_err(|_| RelayControlClientError::Timeout)??;
        Ok(Self {
            deployment_id,
            socket,
        })
    }

    pub async fn sync_policy(
        &mut self,
        runtime: &RelayPolicyRuntime,
    ) -> Result<PolicySync, RelayControlClientError> {
        let request_id = RequestId::new();
        let request = RelayControlClientMessage::GetPolicy {
            request_id,
            deployment_id: self.deployment_id,
            known_policy_version: runtime.policy_version()?,
            node_id: Some(
                std::env::var("PAB_RELAY_NODE_ID").unwrap_or_else(|_| "primary".to_owned()),
            ),
            agent_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        };
        let encoded = serde_json::to_string(&request)?;
        tokio::time::timeout(
            CONTROL_OPERATION_TIMEOUT,
            self.socket.send(Message::Text(encoded.into())),
        )
        .await
        .map_err(|_| RelayControlClientError::Timeout)??;

        let response = tokio::time::timeout(CONTROL_OPERATION_TIMEOUT, self.socket.next())
            .await
            .map_err(|_| RelayControlClientError::Timeout)?
            .ok_or(RelayControlClientError::Closed)??;
        let Message::Text(response) = response else {
            return Err(RelayControlClientError::UnexpectedMessage);
        };
        let response: RelayControlServerMessage = serde_json::from_str(response.as_str())?;
        match response {
            RelayControlServerMessage::PolicySnapshot {
                request_id: response_id,
                snapshot,
            } if response_id == request_id => {
                let version = snapshot.policy_version;
                runtime.apply_snapshot(snapshot)?;
                Ok(PolicySync::Updated {
                    policy_version: version,
                })
            }
            RelayControlServerMessage::PolicyUnchanged {
                request_id: response_id,
                deployment_id,
                policy_version,
                expires_at_unix_ms,
            } if response_id == request_id => {
                runtime.refresh_expiry(deployment_id, policy_version, expires_at_unix_ms)?;
                Ok(PolicySync::Unchanged { policy_version })
            }
            RelayControlServerMessage::Error {
                request_id: Some(response_id),
                code,
                message,
            } if response_id == request_id => {
                Err(RelayControlClientError::Server { code, message })
            }
            _ => Err(RelayControlClientError::MismatchedResponse),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicySync {
    Updated { policy_version: u64 },
    Unchanged { policy_version: u64 },
}

#[derive(Debug, Error)]
pub enum RelayControlClientError {
    #[error("Relay control secret cannot be encoded as an HTTP header")]
    InvalidSecret,
    #[error("Relay control operation timed out")]
    Timeout,
    #[error("Relay control connection closed")]
    Closed,
    #[error("Relay control returned an unexpected WebSocket message")]
    UnexpectedMessage,
    #[error("Relay control response does not match its request")]
    MismatchedResponse,
    #[error("Relay control rejected the request with {code:?}: {message}")]
    Server {
        code: RelayControlErrorCode,
        message: String,
    },
    #[error(transparent)]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Runtime(#[from] PolicyRuntimeError),
}
