use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use pab_protocol::{
    RelayControlClientMessage, RelayControlErrorCode, RelayControlServerMessage, RequestId,
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
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    node_id: String,
    heartbeat_interval: Option<Duration>,
}

impl RelayControlClient {
    pub(crate) async fn retire(
        &mut self,
        boot_id: uuid::Uuid,
    ) -> Result<(), RelayControlClientError> {
        let request_id = RequestId::new();
        let request = RelayControlClientMessage::Goodbye {
            request_id,
            node_id: self.node_id.clone(),
            boot_id,
        };
        match self.exchange(&request).await? {
            RelayControlServerMessage::GoodbyeAccepted { request_id: id } if id == request_id => {
                Ok(())
            }
            _ => Err(RelayControlClientError::UnexpectedMessage),
        }
    }
    pub(crate) fn heartbeat_interval(&self) -> Option<Duration> {
        self.heartbeat_interval
    }
    pub(crate) async fn sync_probe(
        &mut self,
        boot_id: uuid::Uuid,
        runtime: &RelayPolicyRuntime,
    ) -> Result<(), RelayControlClientError> {
        let request_id = RequestId::new();
        let started = std::time::Instant::now();
        let request = RelayControlClientMessage::GetProbe {
            request_id,
            node_id: self.node_id.clone(),
            boot_id,
            applied: runtime.applied_probe(),
        };
        match self.exchange(&request).await? {
            RelayControlServerMessage::Probe {
                request_id: id,
                snapshot,
            } if id == request_id => runtime
                .apply_probe(snapshot, started)
                .map_err(|_| RelayControlClientError::UnexpectedMessage),
            _ => Err(RelayControlClientError::UnexpectedMessage),
        }
    }
    pub(crate) async fn heartbeat(
        &mut self,
        boot_id: uuid::Uuid,
        metrics: &iroh_relay::server::Metrics,
    ) -> Result<(), RelayControlClientError> {
        let request_id = RequestId::new();
        let request = RelayControlClientMessage::Heartbeat {
            request_id,
            node_id: self.node_id.clone(),
            boot_id,
            connections: metrics
                .accepts
                .get()
                .saturating_sub(metrics.disconnects.get()),
            received_bytes: metrics.bytes_recv.get(),
            sent_bytes: metrics.bytes_sent.get(),
        };
        match self.exchange(&request).await? {
            RelayControlServerMessage::HeartbeatAccepted { request_id: id } if id == request_id => {
                Ok(())
            }
            _ => Err(RelayControlClientError::UnexpectedMessage),
        }
    }
    pub(crate) async fn sync_budgets(
        &mut self,
        boot_id: uuid::Uuid,
        runtime: &RelayPolicyRuntime,
    ) -> Result<(), RelayControlClientError> {
        let users = runtime.budget_users();
        if users.is_empty() {
            return Ok(());
        }
        let request_id = RequestId::new();
        let started = std::time::Instant::now();
        let request = RelayControlClientMessage::GetBudgets {
            request_id,
            node_id: self.node_id.clone(),
            boot_id,
            users,
        };
        match self.exchange(&request).await? {
            RelayControlServerMessage::Budgets {
                request_id: id,
                snapshot,
            } if id == request_id => runtime
                .apply_budgets(snapshot, started)
                .map_err(|_| RelayControlClientError::UnexpectedMessage),
            RelayControlServerMessage::Error { code, message, .. } => {
                Err(RelayControlClientError::Server { code, message })
            }
            _ => Err(RelayControlClientError::MismatchedResponse),
        }
    }
    pub async fn cluster_hello(
        &mut self,
        node_id: &str,
        boot_id: uuid::Uuid,
    ) -> Result<(Duration, u64), RelayControlClientError> {
        self.node_id = node_id.to_owned();
        let request_id = RequestId::new();
        let request = RelayControlClientMessage::Hello {
            request_id,
            node_id: node_id.into(),
            boot_id,
            cluster_schema: pab_protocol::RELAY_CLUSTER_SCHEMA_VERSION,
            policy_schema: pab_protocol::RELAY_POLICY_SCHEMA_VERSION,
            agent_version: env!("CARGO_PKG_VERSION").into(),
        };
        let reply = self.exchange(&request).await?;
        match reply {
            RelayControlServerMessage::HelloAccepted {
                request_id: id,
                heartbeat_seconds,
                capacity_bytes_per_second,
            } if id == request_id
                && (1..=30).contains(&heartbeat_seconds)
                && capacity_bytes_per_second > 0 =>
            {
                self.heartbeat_interval = Some(Duration::from_secs(heartbeat_seconds));
                Ok((
                    Duration::from_secs(heartbeat_seconds),
                    capacity_bytes_per_second,
                ))
            }
            RelayControlServerMessage::Error { code, message, .. } => {
                Err(RelayControlClientError::Server { code, message })
            }
            _ => Err(RelayControlClientError::MismatchedResponse),
        }
    }

    async fn exchange(
        &mut self,
        request: &RelayControlClientMessage,
    ) -> Result<RelayControlServerMessage, RelayControlClientError> {
        tokio::time::timeout(CONTROL_OPERATION_TIMEOUT, async {
            self.socket
                .send(Message::Text(serde_json::to_string(request)?.into()))
                .await?;
            loop {
                match self
                    .socket
                    .next()
                    .await
                    .ok_or(RelayControlClientError::Closed)??
                {
                    Message::Text(text) => match serde_json::from_str(&text)? {
                        RelayControlServerMessage::PolicyChanged => {}
                        response => return Ok(response),
                    },
                    Message::Ping(data) => self.socket.send(Message::Pong(data)).await?,
                    Message::Pong(_) => {}
                    _ => return Err(RelayControlClientError::UnexpectedMessage),
                }
            }
        })
        .await
        .map_err(|_| RelayControlClientError::Timeout)?
    }
    pub(crate) async fn report_usage(
        &mut self,
        node_id: &str,
        batch: &pab_protocol::UsageBatch,
    ) -> Result<(), RelayControlClientError> {
        let request_id = RequestId::new();
        let request = RelayControlClientMessage::ReportUsage {
            request_id,
            node_id: node_id.into(),
            batch: batch.clone(),
        };
        tokio::time::timeout(CONTROL_OPERATION_TIMEOUT, async {
            self.socket
                .send(Message::Text(serde_json::to_string(&request)?.into()))
                .await?;
            loop {
                match self
                    .socket
                    .next()
                    .await
                    .ok_or(RelayControlClientError::Closed)??
                {
                    Message::Text(text) => {
                        match serde_json::from_str::<RelayControlServerMessage>(&text)? {
                            RelayControlServerMessage::PolicyChanged => {}
                            RelayControlServerMessage::UsageAccepted { request_id: id }
                                if id == request_id =>
                            {
                                return Ok(());
                            }
                            RelayControlServerMessage::Error { code, message, .. } => {
                                return Err(RelayControlClientError::Server { code, message });
                            }
                            _ => return Err(RelayControlClientError::MismatchedResponse),
                        }
                    }
                    Message::Ping(bytes) => self.socket.send(Message::Pong(bytes)).await?,
                    Message::Pong(_) => {}
                    _ => return Err(RelayControlClientError::Closed),
                }
            }
        })
        .await
        .map_err(|_| RelayControlClientError::Timeout)?
    }
    pub async fn wait_for_policy_change(&mut self) -> Result<(), RelayControlClientError> {
        loop {
            let message = self
                .socket
                .next()
                .await
                .ok_or(RelayControlClientError::Closed)??;
            match message {
                Message::Text(text)
                    if matches!(
                        serde_json::from_str::<RelayControlServerMessage>(&text)?,
                        RelayControlServerMessage::PolicyChanged
                    ) =>
                {
                    return Ok(());
                }
                Message::Ping(_) | Message::Pong(_) => {}
                Message::Close(_) => return Err(RelayControlClientError::Closed),
                _ => return Err(RelayControlClientError::UnexpectedMessage),
            }
        }
    }
    pub async fn connect(
        url: &str,
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
            socket,
            node_id: "primary".into(),
            heartbeat_interval: None,
        })
    }

    pub(crate) fn with_node_id(mut self, node_id: String) -> Self {
        self.node_id = node_id;
        self
    }

    pub async fn sync_policy(
        &mut self,
        runtime: &RelayPolicyRuntime,
    ) -> Result<PolicySync, RelayControlClientError> {
        let started = std::time::Instant::now();
        let request_id = RequestId::new();
        let request = RelayControlClientMessage::GetPolicy {
            request_id,
            known_policy_version: runtime.policy_version()?,
            node_id: Some(self.node_id.clone()),
            agent_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        };
        let encoded = serde_json::to_string(&request)?;
        tokio::time::timeout(
            CONTROL_OPERATION_TIMEOUT,
            self.socket.send(Message::Text(encoded.into())),
        )
        .await
        .map_err(|_| RelayControlClientError::Timeout)??;

        let response = tokio::time::timeout(CONTROL_OPERATION_TIMEOUT, async {
            loop {
                match self
                    .socket
                    .next()
                    .await
                    .ok_or(RelayControlClientError::Closed)??
                {
                    Message::Text(text) => {
                        let response: RelayControlServerMessage = serde_json::from_str(&text)?;
                        if !matches!(response, RelayControlServerMessage::PolicyChanged) {
                            return Ok(response);
                        }
                    }
                    Message::Ping(_) | Message::Pong(_) => {}
                    _ => return Err(RelayControlClientError::UnexpectedMessage),
                }
            }
        })
        .await
        .map_err(|_| RelayControlClientError::Timeout)??;
        match response {
            RelayControlServerMessage::PolicySnapshot {
                request_id: response_id,
                snapshot,
            } if response_id == request_id => {
                let version = snapshot.policy_version;
                runtime.apply_control_snapshot(snapshot, started)?;
                Ok(PolicySync::Updated {
                    policy_version: version,
                })
            }
            RelayControlServerMessage::PolicyUnchanged {
                request_id: response_id,
                policy_version,
                expires_at_unix_ms,
                issued_at_unix_ms,
            } if response_id == request_id => {
                runtime.refresh_control_expiry(
                    policy_version,
                    expires_at_unix_ms,
                    issued_at_unix_ms,
                    started,
                )?;
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
