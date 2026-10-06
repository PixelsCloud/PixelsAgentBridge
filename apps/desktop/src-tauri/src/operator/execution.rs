use std::sync::Arc;

use pab_bridge::BridgeRuntime;
use pab_protocol::{DeviceCode, DeviceRef, RequestId, SystemQuery, SystemQueryReply};
use tauri::State;

use super::{OperatorState, parse_code};

#[derive(serde::Serialize)]
pub struct ExecutionQueryError {
    message: String,
    request_id: RequestId,
    outcome: &'static str,
}

#[derive(Clone)]
pub(super) struct QueryOwner {
    code: DeviceCode,
    device: DeviceRef,
    runtime: Arc<BridgeRuntime>,
}

fn supported(query: &SystemQuery) -> bool {
    matches!(query, SystemQuery::ExecutionContexts { .. } | SystemQuery::Applications { .. })
}

fn terminal(reply: &SystemQueryReply) -> bool {
    matches!(reply.state.as_str(), "completed" | "failed" | "cancelled" | "interrupted")
}

#[tauri::command]
pub async fn operator_execution_query(
    state: State<'_, OperatorState>,
    code: String,
    request_id: RequestId,
    query: SystemQuery,
) -> Result<SystemQueryReply, ExecutionQueryError> {
    let rejected = |message: String| ExecutionQueryError { message, request_id, outcome: "not_submitted" };
    if !supported(&query) { return Err(rejected("unsupported execution panel query".into())); }
    query.validate().map_err(|e| rejected(e.into()))?;
    let code = parse_code(&code).map_err(rejected)?;
    let runtime = state.runtime().await.map_err(rejected)?;
    let device = runtime.resolve_device_code(code).await.map_err(|e| rejected(e.to_string()))?;
    {
        let mut pending = state.execution_queries.lock().await;
        if let Some(owner) = pending.get(&request_id) {
            if owner.code != code || owner.device != device || !Arc::ptr_eq(&owner.runtime, &runtime) {
                return Err(rejected("query belongs to another device or account connection".into()));
            }
        } else {
            if pending.len() >= 128 { return Err(rejected("too many unresolved execution queries; inspect existing operations".into())); }
            pending.insert(request_id, QueryOwner { code, device, runtime: Arc::clone(&runtime) });
        }
    }
    // On an unknown submission outcome, retain the exact owning runtime for
    // observation. Account changes must never redirect an in-flight operation.
    let reply = runtime.system_query(device, request_id, query).await.map_err(|e| ExecutionQueryError {
        message: e.to_string(), request_id, outcome: "unconfirmed",
    })?;
    if terminal(&reply) { state.execution_queries.lock().await.remove(&request_id); }
    Ok(reply)
}

#[tauri::command]
pub async fn operator_execution_query_result(
    state: State<'_, OperatorState>,
    code: String,
    request_id: RequestId,
) -> Result<SystemQueryReply, String> {
    let code = parse_code(&code)?;
    let owner = state.execution_queries.lock().await.get(&request_id).cloned()
        .ok_or("query is no longer pending; inspect task history")?;
    if owner.code != code { return Err("query belongs to another device".into()); }
    let reply = owner.runtime.get_system_query(owner.device, request_id).await.map_err(|e| e.to_string())?;
    if terminal(&reply) { state.execution_queries.lock().await.remove(&request_id); }
    Ok(reply)
}
