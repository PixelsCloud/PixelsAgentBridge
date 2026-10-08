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
    let mut changes = state.control.web_changes();
    loop {
        let next = tokio::select! {
            next = tokio::time::timeout(RELAY_IDLE_TIMEOUT, receiver.next()) => next,
            changed = changes.changed() => {
                if changed.is_err() { break; }
                let encoded = serde_json::to_string(&RelayControlServerMessage::PolicyChanged).expect("policy notification");
                if sender.send(Message::Text(encoded.into())).await.is_err() { break; }
                continue;
            }
        };
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
            known_policy_version,
            node_id,
            agent_version,
        } => {
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
            if let Some(node_id) = node_id {
                if node_id.is_empty()
                    || node_id.len() > 64
                    || !node_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
                    || agent_version
                        .as_ref()
                        .is_some_and(|s| s.len() > 64 || s.chars().any(char::is_control))
                {
                    return error(
                        Some(request_id),
                        RelayControlErrorCode::InvalidMessage,
                        "invalid Relay node metadata",
                    );
                }
                if known_policy_version.is_none_or(|v| v <= snapshot.policy_version) {
                    let applied = known_policy_version.and_then(|v| i64::try_from(v).ok());
                    let Ok(offered) = i64::try_from(snapshot.policy_version) else {
                        return error(
                            Some(request_id),
                            RelayControlErrorCode::Internal,
                            "invalid policy version",
                        );
                    };
                    if sqlx::query("INSERT INTO relay_nodes(node_id,agent_version,applied_policy_version,offered_policy_version,server_instance) VALUES($1,$2,$3,$4,$5) ON CONFLICT(node_id) DO UPDATE SET agent_version=excluded.agent_version,applied_policy_version=excluded.applied_policy_version,offered_policy_version=excluded.offered_policy_version,last_seen_at=clock_timestamp(),server_instance=excluded.server_instance")
                        .bind(node_id).bind(agent_version).bind(applied).bind(offered).bind(state.server_instance).execute(state.control.store().pool()).await.is_err(){
                        tracing::warn!("could not record Relay node status");
                    }
                }
            }
            match known_policy_version {
                Some(version) if version > snapshot.policy_version => error(
                    Some(request_id),
                    RelayControlErrorCode::InvalidPolicyVersion,
                    "Relay policy version is ahead of the server",
                ),
                Some(version) if version == snapshot.policy_version => {
                    RelayControlServerMessage::PolicyUnchanged {
                        request_id,
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
