use super::{WebError, WebState, management::administrator};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use pab_protocol::EndpointKey;
use sqlx::Row;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProbeInput {
    first: EndpointKey,
    second: EndpointKey,
}
pub(crate) async fn create(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(node): Path<String>,
    Json(input): Json<ProbeInput>,
) -> Result<Json<serde_json::Value>, WebError> {
    administrator(&state, &headers).await?;
    let config = state.control.relay_config().ok_or_else(WebError::invalid)?;
    let configured = config
        .nodes
        .iter()
        .find(|n| n.node_id == node)
        .ok_or_else(WebError::invalid)?;
    if input.first == input.second
        || iroh_base::PublicKey::from_bytes(input.first.as_bytes()).is_err()
        || iroh_base::PublicKey::from_bytes(input.second.as_bytes()).is_err()
    {
        return Err(WebError::invalid());
    }
    // Probe admission must never grant access to an existing real endpoint.
    let registered: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM endpoints WHERE endpoint_key=ANY($1::bytea[]))",
    )
    .bind(vec![
        input.first.as_bytes().to_vec(),
        input.second.as_bytes().to_vec(),
    ])
    .fetch_one(state.control.store().pool())
    .await?;
    if registered {
        return Err(WebError::new(
            StatusCode::CONFLICT,
            "probe_endpoint_registered",
        ));
    }
    let row=sqlx::query("SELECT boot_id FROM relay_cluster_nodes WHERE node_id=$1 AND configured AND admin_state<>'disabled' AND health='healthy' AND last_seen_at>clock_timestamp()-($2::bigint*interval '1 second')")
        .bind(&node).bind(config.offline_seconds as i64).fetch_optional(state.control.store().pool()).await?.ok_or_else(||WebError::new(StatusCode::CONFLICT,"relay_not_ready"))?;
    let boot: Option<uuid::Uuid> = row.try_get("boot_id")?;
    let boot = boot.ok_or_else(WebError::invalid)?;
    let mut probes = state
        .control
        .relay_probes
        .lock()
        .map_err(|_| WebError::new(StatusCode::INTERNAL_SERVER_ERROR, "server_error"))?;
    let id = probes
        .create(
            &node,
            boot,
            input.first,
            input.second,
            configured.url.clone(),
        )
        .map_err(|_| WebError::new(StatusCode::CONFLICT, "probe_busy"))?;
    Ok(Json(probes.status(id).unwrap()))
}
pub(crate) async fn status(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, WebError> {
    administrator(&state, &headers).await?;
    let probes = state
        .control
        .relay_probes
        .lock()
        .map_err(|_| WebError::new(StatusCode::INTERNAL_SERVER_ERROR, "server_error"))?;
    Ok(Json(probes.status(id).ok_or_else(|| {
        WebError::new(StatusCode::NOT_FOUND, "not_found")
    })?))
}
pub(crate) async fn remove(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Result<StatusCode, WebError> {
    administrator(&state, &headers).await?;
    state
        .control
        .relay_probes
        .lock()
        .map_err(|_| WebError::new(StatusCode::INTERNAL_SERVER_ERROR, "server_error"))?
        .remove(id);
    Ok(StatusCode::NO_CONTENT)
}
