use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use pab_protocol::{
    RelayControlClientMessage, RelayControlErrorCode, RelayControlServerMessage, RequestId,
};

use super::ControlApiState;

const RELAY_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

pub(super) async fn run_relay_socket(
    socket: WebSocket,
    state: ControlApiState,
    identity: Option<String>,
    binding: [u8; 32],
) {
    let (mut sender, mut receiver) = socket.split();
    let mut changes = state.control.web_changes();
    let mut boot_id = None;
    let mut legacy_reporter = super::relay_telemetry::LegacyReporter::new();
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
                Ok(message) => {
                    handle_message(
                        &state,
                        identity.as_deref(),
                        binding,
                        &mut boot_id,
                        &mut legacy_reporter,
                        message,
                    )
                    .await
                }
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
    identity: Option<&str>,
    binding: [u8; 32],
    boot: &mut Option<uuid::Uuid>,
    legacy_reporter: &mut super::relay_telemetry::LegacyReporter,
    message: RelayControlClientMessage,
) -> RelayControlServerMessage {
    if let Some(expected) = identity {
        let authorized = state.control.relay_config().is_some_and(|config| {
            config.nodes.iter().any(|node| {
                node.node_id == expected
                    && *blake3::hash(node.control_secret.as_bytes()).as_bytes() == binding
            })
        });
        if !authorized {
            return error(
                Some(message.request_id()),
                RelayControlErrorCode::InvalidMessage,
                "Relay credential was revoked",
            );
        }
        let claimed = match &message {
            RelayControlClientMessage::GetNodeControl { node_id, .. } => Some(node_id.as_str()),
            RelayControlClientMessage::Goodbye { node_id, .. } => Some(node_id.as_str()),
            RelayControlClientMessage::GetProbe { node_id, .. } => Some(node_id.as_str()),
            RelayControlClientMessage::Hello { node_id, .. }
            | RelayControlClientMessage::Heartbeat { node_id, .. }
            | RelayControlClientMessage::GetBudgets { node_id, .. }
            | RelayControlClientMessage::ReportUsage { node_id, .. } => Some(node_id.as_str()),
            RelayControlClientMessage::GetPolicy { node_id, .. } => node_id.as_deref(),
        };
        if claimed != Some(expected)
            || (boot.is_none() && !matches!(message, RelayControlClientMessage::Hello { .. }))
        {
            return error(
                Some(message.request_id()),
                RelayControlErrorCode::InvalidMessage,
                "Relay node identity/handshake mismatch",
            );
        }
    }
    if identity.is_none() && state.control.relay_config().is_some() {
        return error(
            Some(message.request_id()),
            RelayControlErrorCode::InvalidMessage,
            "legacy Relay is not allowed in cluster mode",
        );
    }
    match message {
        RelayControlClientMessage::GetNodeControl {request_id,node_id,boot_id,connections,applied} => {
            if identity!=Some(node_id.as_str()) || *boot!=Some(boot_id) {
                return error(Some(request_id),RelayControlErrorCode::InvalidMessage,"Relay node control identity mismatch");
            }
            let Some(config)=state.control.relay_config() else { return error(Some(request_id),RelayControlErrorCode::InvalidMessage,"Relay directory unavailable"); };
            match state.control.store().relay_node_control(&node_id,boot_id,state.server_instance,&config,applied.as_ref(),connections).await {
                Ok(snapshot)=>RelayControlServerMessage::NodeControl {request_id,snapshot},
                Err(_)=>error(Some(request_id),RelayControlErrorCode::InvalidMessage,"Relay node control unavailable"),
            }
        }
        RelayControlClientMessage::Goodbye {
            request_id,
            node_id,
            boot_id,
        } => {
            if identity != Some(node_id.as_str()) || *boot != Some(boot_id) {
                return error(
                    Some(request_id),
                    RelayControlErrorCode::InvalidMessage,
                    "Relay retirement identity mismatch",
                );
            }
            match state.control.store().retire_relay(&node_id, boot_id).await {
                Ok(()) => RelayControlServerMessage::GoodbyeAccepted { request_id },
                Err(_) => error(
                    Some(request_id),
                    RelayControlErrorCode::InvalidMessage,
                    "Relay process is no longer active",
                ),
            }
        }
        RelayControlClientMessage::GetProbe {
            request_id,
            node_id,
            boot_id,
            applied,
        } => {
            if identity != Some(node_id.as_str()) || *boot != Some(boot_id) {
                return error(
                    Some(request_id),
                    RelayControlErrorCode::InvalidMessage,
                    "Relay probe identity mismatch",
                );
            }
            match state.control.relay_probes.lock() {
                Ok(mut probes) => RelayControlServerMessage::Probe {
                    request_id,
                    snapshot: probes.snapshot(&node_id, boot_id, applied),
                },
                Err(_) => error(
                    Some(request_id),
                    RelayControlErrorCode::Internal,
                    "Relay probe state unavailable",
                ),
            }
        }
        RelayControlClientMessage::Heartbeat {
            request_id,
            node_id,
            boot_id,
            connections,
            received_bytes,
            sent_bytes,
            diagnostics,
        } => {
            if identity != Some(node_id.as_str())
                || *boot != Some(boot_id)
                || [connections, received_bytes, sent_bytes]
                    .iter()
                    .any(|v| *v > i64::MAX as u64)
            {
                return error(
                    Some(request_id),
                    RelayControlErrorCode::InvalidMessage,
                    "invalid Relay telemetry",
                );
            }
            match sqlx::query("UPDATE relay_cluster_nodes SET connections=$3,uploaded_bytes=$4,downloaded_bytes=$5 WHERE node_id=$1 AND boot_id=$2 AND configured")
                .bind(&node_id).bind(boot_id).bind(connections as i64).bind(received_bytes as i64).bind(sent_bytes as i64)
                .execute(state.control.store().pool()).await {
                Ok(result) if result.rows_affected()==1=>{
                    if let Some(sample)=diagnostics {
                        if !state.relay_diagnostics.record(&node_id,boot_id,sample){tracing::warn!(node=%node_id,"Relay diagnostics unavailable");}
                    }
                    RelayControlServerMessage::HeartbeatAccepted{request_id}
                },
                _=>error(Some(request_id),RelayControlErrorCode::InvalidMessage,"Relay telemetry process is no longer active"),
            }
        }
        RelayControlClientMessage::GetBudgets {
            request_id,
            node_id,
            boot_id,
            users,
        } => {
            if identity != Some(node_id.as_str()) || *boot != Some(boot_id) {
                return error(
                    Some(request_id),
                    RelayControlErrorCode::InvalidMessage,
                    "Relay budget identity mismatch",
                );
            }
            let Some(config) = state.control.relay_config() else {
                return error(
                    Some(request_id),
                    RelayControlErrorCode::InvalidMessage,
                    "cluster mode is not enabled",
                );
            };
            let database_ms = sqlx::query_scalar::<_, i64>(
                "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint",
            )
            .fetch_one(state.control.store().pool())
            .await;
            let stable = database_ms.ok().is_some_and(|ms| {
                state
                    .control
                    .relay_budget_clock
                    .lock()
                    .is_ok_and(|mut clock| clock.observe(ms, std::time::Instant::now()))
            });
            if !stable {
                return error(
                    Some(request_id),
                    RelayControlErrorCode::Internal,
                    "Relay budget recovery is waiting for previous leases to expire",
                );
            }
            match state
                .control
                .store()
                .relay_budgets(&node_id, boot_id, &users, &config)
                .await
            {
                Ok(snapshot) => RelayControlServerMessage::Budgets {
                    request_id,
                    snapshot,
                },
                Err(_) => error(
                    Some(request_id),
                    RelayControlErrorCode::InvalidMessage,
                    "Relay budget request is invalid or node is not ready",
                ),
            }
        }
        RelayControlClientMessage::Hello {
            request_id,
            node_id,
            boot_id,
            cluster_schema,
            policy_schema,
            agent_version,
        } => {
            let Some(config) = state.control.relay_config() else {
                return error(
                    Some(request_id),
                    RelayControlErrorCode::InvalidMessage,
                    "cluster mode is not enabled",
                );
            };
            if identity != Some(node_id.as_str())
                || boot_id.is_nil()
                || agent_version.len() > 64
                || agent_version.chars().any(char::is_control)
            {
                return error(
                    Some(request_id),
                    RelayControlErrorCode::InvalidMessage,
                    "invalid Relay handshake",
                );
            }
            let compatible = cluster_schema == pab_protocol::RELAY_CLUSTER_SCHEMA_VERSION
                && policy_schema == pab_protocol::RELAY_POLICY_SCHEMA_VERSION;
            match state
                .control
                .store()
                .register_relay_boot(
                    &node_id,
                    boot_id,
                    &agent_version,
                    compatible,
                    &config,
                    state.server_instance,
                )
                .await
            {
                Ok(()) if compatible => {
                    *boot = Some(boot_id);
                    let capacity_bytes_per_second = config
                        .nodes
                        .iter()
                        .find(|n| n.node_id == node_id)
                        .map(|n| u64::from(n.capacity_mbps) * 1_000_000 / 8)
                        .unwrap_or(0);
                    RelayControlServerMessage::HelloAccepted {
                        node_control_version: Some(pab_protocol::RELAY_NODE_CONTROL_VERSION),
                        diagnostics_version: Some(pab_protocol::RELAY_DIAGNOSTICS_VERSION),
                        request_id,
                        heartbeat_seconds: config.heartbeat_seconds,
                        capacity_bytes_per_second,
                    }
                }
                Err(crate::StoreError::Conflict(_)) if compatible => error(
                    Some(request_id),
                    RelayControlErrorCode::NodeBusy,
                    "Relay node still has a current process; wait for its heartbeat to expire",
                ),
                _ => error(
                    Some(request_id),
                    RelayControlErrorCode::InvalidMessage,
                    "Relay handshake rejected: identity, active process or protocol mismatch",
                ),
            }
        }
        RelayControlClientMessage::ReportUsage {
            request_id,
            node_id,
            batch,
        } => {
            if node_id.is_empty()
                || node_id.len() > 64
                || !node_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
                || batch.counters.values()[2..].iter().any(|v| *v != 0)
                || batch.counters.p2p_reported
            {
                return error(
                    Some(request_id),
                    RelayControlErrorCode::InvalidMessage,
                    "invalid Relay usage",
                );
            }
            match state
                .control
                .store()
                .record_usage("relay", &node_id, &batch)
                .await
            {
                Ok(()) => RelayControlServerMessage::UsageAccepted { request_id },
                Err(crate::StoreError::InvalidInput(_) | crate::StoreError::Conflict(_)) => error(
                    Some(request_id),
                    RelayControlErrorCode::InvalidMessage,
                    "invalid or conflicting usage batch",
                ),
                Err(_) => error(
                    Some(request_id),
                    RelayControlErrorCode::Internal,
                    "usage storage unavailable",
                ),
            }
        }
        RelayControlClientMessage::GetPolicy {
            request_id,
            known_policy_version,
            node_id,
            agent_version,
            telemetry,
        } => {
            if let Some(sample) = telemetry.as_ref() {
                if identity.is_some() || !legacy_reporter.validate(node_id.as_deref(), sample) {
                    return error(
                        Some(request_id),
                        RelayControlErrorCode::InvalidMessage,
                        "invalid Relay telemetry",
                    );
                }
            }
            let head = sqlx::query_scalar::<_, i64>(
                "SELECT policy_revision FROM server_settings WHERE singleton",
            )
            .fetch_one(state.control.store().pool())
            .await;
            let Ok(head) = head else {
                return error(
                    Some(request_id),
                    RelayControlErrorCode::Internal,
                    "Relay policy is unavailable",
                );
            };
            if known_policy_version.is_some_and(|version| version > head as u64) {
                return error(
                    Some(request_id),
                    RelayControlErrorCode::InvalidPolicyVersion,
                    "Relay policy version is ahead of the server",
                );
            }
            if let (Some(node), Some(boot_id), Some(config)) =
                (identity, *boot, state.control.relay_config())
            {
                if state
                    .control
                    .store()
                    .heartbeat_relay(
                        node,
                        boot_id,
                        known_policy_version.is_some_and(|v| v > 0),
                        &config,
                    )
                    .await
                    .is_err()
                {
                    return error(
                        Some(request_id),
                        RelayControlErrorCode::InvalidMessage,
                        "Relay process is no longer active",
                    );
                }
            }
            if known_policy_version == Some(head as u64) {
                if let Some(node_id) = node_id.as_deref() {
                    if !pab_protocol::valid_relay_node_id(node_id)
                        || agent_version
                            .as_ref()
                            .is_some_and(|v| v.len() > 64 || v.chars().any(char::is_control))
                    {
                        return error(
                            Some(request_id),
                            RelayControlErrorCode::InvalidMessage,
                            "invalid Relay node metadata",
                        );
                    }
                    if sqlx::query("INSERT INTO relay_nodes(node_id,agent_version,applied_policy_version,offered_policy_version,server_instance) VALUES($1,$2,$3,$3,$4) ON CONFLICT(node_id) DO UPDATE SET agent_version=excluded.agent_version,applied_policy_version=excluded.applied_policy_version,offered_policy_version=excluded.offered_policy_version,last_seen_at=clock_timestamp(),server_instance=excluded.server_instance")
                        .bind(node_id).bind(&agent_version).bind(head).bind(state.server_instance).execute(state.control.store().pool()).await.is_err() {
                        return error(Some(request_id),RelayControlErrorCode::Internal,"Relay status could not be recorded");
                    }
                }
                let expires = std::time::SystemTime::now()
                    .checked_add(state.config.relay_policy_validity)
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .and_then(|time| i64::try_from(time.as_millis()).ok());
                if let (Some(node), Some(sample)) = (node_id.as_deref(), telemetry.as_ref()) {
                    legacy_reporter.record(state, node, sample).await;
                }
                return match expires {
                    Some(expires_at_unix_ms) => RelayControlServerMessage::PolicyUnchanged {
                        request_id,
                        policy_version: head as u64,
                        expires_at_unix_ms,
                        issued_at_unix_ms: identity.map(|_| {
                            expires_at_unix_ms.saturating_sub(
                                state.config.relay_policy_validity.as_millis() as i64,
                            )
                        }),
                    },
                    None => error(
                        Some(request_id),
                        RelayControlErrorCode::Internal,
                        "Relay policy expiry is invalid",
                    ),
                };
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
            if let Some(node_id) = node_id.as_deref() {
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
            if let (Some(node), Some(sample)) = (node_id.as_deref(), telemetry.as_ref()) {
                legacy_reporter.record(state, node, sample).await;
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
                        issued_at_unix_ms: identity.map(|_| snapshot.issued_at_unix_ms),
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
