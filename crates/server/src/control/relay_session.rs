use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use pab_protocol::{
    RelayControlClientMessage, RelayControlErrorCode, RelayControlServerMessage, RequestId,
};

use super::ControlApiState;

const RELAY_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

pub(super) async fn run_relay_socket(socket: WebSocket, state: ControlApiState) {
    let (mut sender, mut receiver) = socket.split();
    loop {
        let next = tokio::time::timeout(RELAY_IDLE_TIMEOUT, receiver.next()).await;
        let message = match next {
            Ok(Some(Ok(message))) => message,
            Ok(Some(Err(_))) | Ok(None) | Err(_) => break,
        };
        let response = match message {
            Message::Text(text) => match serde_json::from_str(text.as_str()) {
                Ok(message) => handle_message(&state, message).await,
                Err(_) => error(
                    None,
                    RelayControlErrorCode::InvalidMessage,
                    "invalid Relay control message",
                ),
            },
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => break,
            Message::Binary(_) => error(
                None,
                RelayControlErrorCode::InvalidMessage,
                "Relay control messages must use text JSON frames",
            ),
        };
        let Ok(encoded) = serde_json::to_string(&response) else {
            break;
        };
        if sender.send(Message::Text(encoded.into())).await.is_err() {
            break;
        }
    }
}

async fn handle_message(
    state: &ControlApiState,
    message: RelayControlClientMessage,
) -> RelayControlServerMessage {
    match message {
        RelayControlClientMessage::GetPolicy {
            request_id,
            deployment_id,
            known_policy_version,
        } => {
            if deployment_id != state.deployment_id {
                return error(
                    Some(request_id),
                    RelayControlErrorCode::InvalidDeployment,
                    "Relay belongs to a different deployment",
                );
            }
            let snapshot = match state
                .control
                .relay_policy_snapshot(state.config.relay_policy_validity)
                .await
            {
                Ok(snapshot) => snapshot,
                Err(_) => {
                    return error(
                        Some(request_id),
                        RelayControlErrorCode::Internal,
                        "Relay policy could not be loaded",
                    );
                }
            };
            match known_policy_version {
                Some(version) if version > snapshot.policy_version => error(
                    Some(request_id),
                    RelayControlErrorCode::InvalidPolicyVersion,
                    "Relay policy version is ahead of the server",
                ),
                Some(version) if version == snapshot.policy_version => {
                    RelayControlServerMessage::PolicyUnchanged {
                        request_id,
                        deployment_id: snapshot.deployment_id,
                        policy_version: snapshot.policy_version,
                        expires_at_unix_ms: snapshot.expires_at_unix_ms,
                    }
                }
                _ => RelayControlServerMessage::PolicySnapshot {
                    request_id,
                    snapshot,
                },
            }
        }
    }
}

fn error(
    request_id: Option<RequestId>,
    code: RelayControlErrorCode,
    message: &str,
) -> RelayControlServerMessage {
    RelayControlServerMessage::Error {
        request_id,
        code,
        message: message.to_owned(),
    }
}
