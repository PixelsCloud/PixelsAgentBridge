use super::{
    WebError, WebState,
    management::{PageQuery, administrator},
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use pab_protocol::{EndpointKey, RELAY_LINK_ACTIVE_MS};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use std::collections::{HashMap, HashSet};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeQuery {
    view: Option<String>,
    status: Option<String>,
    page: Option<u32>,
    page_size: Option<u32>,
    q: Option<String>,
}
fn key(endpoint: EndpointKey) -> String {
    endpoint
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn identity(endpoint: EndpointKey, identities: &HashMap<String, Value>) -> Value {
    let key = key(endpoint);
    identities
        .get(&key)
        .cloned()
        .unwrap_or_else(|| json!({"endpoint":key,"kind":"unknown"}))
}
fn timestamp(sample: i64, age: u64) -> i64 {
    sample.saturating_sub(i64::try_from(age).unwrap_or(i64::MAX))
}
fn matches_search(row: &Value, search: &str) -> bool {
    ["identity", "a", "b"].into_iter().any(|side| {
        [
            "endpoint",
            "name",
            "code",
            "username",
            "system",
            "os_name",
            "os_version",
            "architecture",
            "agent_version",
        ]
        .into_iter()
        .any(|field| {
            row[side][field]
                .as_str()
                .is_some_and(|value| value.to_lowercase().contains(search))
        })
    })
}

pub(super) async fn get(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(node): Path<String>,
    Query(query): Query<RuntimeQuery>,
) -> Result<Json<Value>, WebError> {
    administrator(&state, &headers).await?;
    if !pab_protocol::valid_relay_node_id(&node) {
        return Err(WebError::invalid());
    }
    let view = query.view.as_deref().unwrap_or("connections");
    let status = query.status.as_deref().unwrap_or("all");
    if !["connections", "links"].contains(&view)
        || !["all", "connected", "active", "idle", "closed"].contains(&status)
        || (view == "connections" && ["active", "idle"].contains(&status))
        || (view == "links" && status == "connected")
    {
        return Err(WebError::invalid());
    }
    let (page, size, search) = PageQuery {
        page: query.page,
        page_size: query.page_size,
        q: query.q.clone(),
    }
    .values()
    .map(|(p, s, q)| (p, s, q.to_lowercase()))?;
    let cluster = state.control.relay_config();
    let sql = if cluster.is_some() {
        "SELECT n.boot_id current_boot,n.server_instance current_server,n.configured AND NOT n.retired AND n.last_seen_at>clock_timestamp()-($2::bigint*interval '1 second') current_node FROM relay_cluster_nodes n WHERE n.node_id=$1"
    } else {
        "SELECT n.telemetry_boot_id current_boot,n.server_instance current_server,n.last_seen_at>clock_timestamp()-($2::bigint*interval '1 second') current_node FROM relay_nodes n WHERE n.node_id=$1"
    };
    let offline = cluster.as_ref().map_or(120, |v| v.offline_seconds);
    let row = sqlx::query(sql)
        .bind(&node)
        .bind(offline as i64)
        .fetch_optional(state.control.store().pool())
        .await?
        .ok_or_else(|| WebError::new(StatusCode::NOT_FOUND, "not_found"))?;
    let Some(sample) = state.relay_diagnostics.get(&node) else {
        return Ok(Json(
            json!({"supported":false,"fresh":false,"items":[],"total":0,"page":page,"page_size":size}),
        ));
    };
    let snapshot = &sample.snapshot;
    let sampled = sample.received_at_ms;
    let age = sample.received.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let ttl = cluster
        .as_ref()
        .map_or(60_000, |v| (v.heartbeat_seconds * 3000).max(15_000));
    let fresh = row.try_get::<Option<uuid::Uuid>, _>("current_server")?
        == Some(state.server_instance)
        && Some(sample.boot) == row.try_get::<Option<uuid::Uuid>, _>("current_boot")?
        && row
            .try_get::<Option<bool>, _>("current_node")?
            .unwrap_or(false)
        && age <= ttl;
    let endpoints: HashSet<EndpointKey> = snapshot
        .connections
        .iter()
        .map(|r| r.endpoint)
        .chain(snapshot.links.iter().flat_map(|r| [r.a, r.b]))
        .collect();
    let keys: Vec<Vec<u8>> = endpoints
        .into_iter()
        .map(|key| key.as_bytes().to_vec())
        .collect();
    let rows=sqlx::query("SELECT encode(e.endpoint_key,'hex') endpoint,e.owner_kind,d.id device_id,d.name,d.code,COALESCE(u.username,owner.username) username,r.execution_context->>'os_family' system,r.execution_context->>'os_name' os_name,r.execution_context->>'os_version' os_version,r.execution_context->>'architecture' architecture,r.agent_version FROM endpoints e LEFT JOIN users u ON u.id=e.user_id LEFT JOIN devices d ON d.id=e.device_id AND d.tenant_id=e.tenant_id LEFT JOIN device_accounts p ON p.device_id=d.id LEFT JOIN users owner ON owner.id=p.user_id LEFT JOIN device_runtime r ON r.device_id=d.id AND r.tenant_id=d.tenant_id WHERE e.endpoint_key=ANY($1::bytea[])")
        .bind(keys).fetch_all(state.control.store().pool()).await?;
    let mut identities = HashMap::new();
    for row in rows {
        let endpoint: String = row.try_get("endpoint")?;
        identities.insert(endpoint.clone(),json!({"endpoint":endpoint,"kind":row.try_get::<String,_>("owner_kind")?,"device_id":row.try_get::<Option<uuid::Uuid>,_>("device_id")?,"name":row.try_get::<Option<String>,_>("name")?,"code":row.try_get::<Option<i32>,_>("code")?.map(|n|format!("{n:09}")),"username":row.try_get::<Option<String>,_>("username")?,"system":row.try_get::<Option<String>,_>("system")?,"os_name":row.try_get::<Option<String>,_>("os_name")?,"os_version":row.try_get::<Option<String>,_>("os_version")?,"architecture":row.try_get::<Option<String>,_>("architecture")?,"agent_version":row.try_get::<Option<String>,_>("agent_version")?}));
    }
    let active = snapshot
        .links
        .iter()
        .filter(|r| {
            r.closed_ago_ms.is_none()
                && r.last_forwarded_ago_ms.saturating_add(age) <= RELAY_LINK_ACTIVE_MS
        })
        .count();
    let idle = snapshot
        .links
        .iter()
        .filter(|r| {
            r.closed_ago_ms.is_none()
                && r.last_forwarded_ago_ms.saturating_add(age) > RELAY_LINK_ACTIVE_MS
        })
        .count();
    let connected_endpoints = snapshot
        .connections
        .iter()
        .filter(|r| r.closed_ago_ms.is_none())
        .map(|r| r.endpoint)
        .collect::<HashSet<_>>()
        .len();
    let mut items: Vec<Value> = if view == "connections" {
        snapshot.connections.iter().map(|r|{
        let status=if r.closed_ago_ms.is_some(){"closed"}else if fresh{"connected"}else{"unknown"};
        let mut identity=identity(r.endpoint,&identities);if r.probe{identity["kind"]=json!("probe");}
        json!({"id":r.id,"identity":identity,"status":status,"connected_at":timestamp(sampled,r.age_ms),"duration_ms":r.connected_ms.saturating_add(if fresh&&r.closed_ago_ms.is_none(){age}else{0}).to_string(),"closed_at":r.closed_ago_ms.map(|v|timestamp(sampled,v))})
    }).collect()
    } else {
        snapshot.links.iter().map(|r|{
        let status=if r.closed_ago_ms.is_some(){"closed"}else if !fresh{"unknown"}else if r.last_forwarded_ago_ms.saturating_add(age)<=RELAY_LINK_ACTIVE_MS{"active"}else{"idle"};
        json!({"id":r.id,"a":identity(r.a,&identities),"b":identity(r.b,&identities),"status":status,"started_at":timestamp(sampled,r.age_ms),"last_forwarded_at":timestamp(sampled,r.last_forwarded_ago_ms),"duration_ms":r.observed_ms.saturating_add(if fresh&&r.closed_ago_ms.is_none(){age}else{0}).to_string(),"closed_at":r.closed_ago_ms.map(|v|timestamp(sampled,v)),"a_to_b_bytes":r.a_to_b_bytes.to_string(),"b_to_a_bytes":r.b_to_a_bytes.to_string(),"a_to_b_packets":r.a_to_b_packets.to_string(),"b_to_a_packets":r.b_to_a_packets.to_string(),"a_to_b_rate":(if fresh&&status=="active"{r.a_to_b_bytes_per_second}else{0}).to_string(),"b_to_a_rate":(if fresh&&status=="active"{r.b_to_a_bytes_per_second}else{0}).to_string()})
    }).collect()
    };
    items.retain(|r| {
        (status == "all" || r["status"] == status)
            && (search.is_empty() || matches_search(r, &search))
    });
    let total = items.len();
    let items: Vec<Value> = items
        .into_iter()
        .skip(((page - 1) * size) as usize)
        .take(size as usize)
        .collect();
    let mut host = serde_json::to_value(&snapshot.host).map_err(|_| WebError::invalid())?;
    let mut traffic = serde_json::to_value(&snapshot.traffic).map_err(|_| WebError::invalid())?;
    for value in traffic
        .as_object_mut()
        .ok_or_else(WebError::invalid)?
        .values_mut()
    {
        if let Some(n) = value.as_u64() {
            *value = json!(n.to_string());
        }
    }
    for field in [
        "memory_total",
        "memory_used",
        "process_memory",
        "host_uptime_ms",
        "process_uptime_ms",
    ] {
        if let Some(v) = host[field].as_u64() {
            host[field] = json!(v.to_string());
        }
    }
    Ok(Json(
        json!({"supported":true,"fresh":fresh,"sampled_at":sampled,"sample_age_ms":age,"boot_id":sample.boot,"host":host,"traffic":traffic,"summary":{"connections":if fresh{Some(snapshot.connections_total.to_string())}else{None},"connected_endpoints":if fresh&&!snapshot.connections_truncated{Some(connected_endpoints)}else{None},"active_links":if fresh&&!snapshot.links_truncated{Some(active)}else{None},"idle_links":if fresh&&!snapshot.links_truncated{Some(idle)}else{None},"recent_closed_links":snapshot.links.iter().filter(|r|r.closed_ago_ms.is_some()).count(),"connections_truncated":snapshot.connections_truncated,"links_truncated":snapshot.links_truncated},"items":items,"total":total,"page":page,"page_size":size,"active_window_ms":RELAY_LINK_ACTIVE_MS}),
    ))
}
