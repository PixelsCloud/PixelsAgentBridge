//! One snapshot and one classification for Relay lists and overview counters.
use super::{WebError, WebState, management::PageQuery};
use serde::Deserialize;
use serde_json::Value;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RelayQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    q: Option<String>,
    status: Option<RelayStatus>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RelayStatus {
    Configured,
    Available,
    Maintenance,
    Disabled,
    Abnormal,
    History,
}

impl RelayStatus {
    fn name(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Available => "available",
            Self::Maintenance => "maintenance",
            Self::Disabled => "disabled",
            Self::Abnormal => "abnormal",
            Self::History => "history",
        }
    }
}

pub(super) async fn snapshot(state: &WebState, query: RelayQuery) -> Result<Value, WebError> {
    let started = std::time::Instant::now();
    let pagination = PageQuery {
        page: query.page,
        page_size: query.page_size,
        q: query.q,
    };
    let (page, size, search) = pagination.values()?;
    let config = state.control.relay_config();
    let offline_seconds = config.as_ref().map_or(120, |value| value.offline_seconds);
    // These statements retain legacy fields. Exact counters are additive, so old
    // native admin clients remain compatible without rounding large integers.
    let nodes = if config.is_some() {
        r#"SELECT n.node_id id,n.public_url url,host(n.machine_ip) machine_ip,n.region,n.weight,n.capacity_mbps,CASE WHEN n.capacity_mbps=0 THEN 'host' ELSE 'configured' END::text capacity_mode,n.configured,
          n.admin_state,CASE WHEN n.health IN ('healthy','starting','suspect') AND n.last_seen_at<=s.at-($2::bigint*interval '1 second') THEN 'offline'
            WHEN n.health IN ('healthy','starting','suspect') AND (n.last_seen_at IS NULL OR n.server_instance IS DISTINCT FROM $1) THEN 'starting' ELSE n.health END health,
          n.health='healthy' AND COALESCE(n.server_instance=$1,false) AND COALESCE(n.last_seen_at>s.at-($2::bigint*interval '1 second'),false) online,
          COALESCE(n.server_instance=$1,false) sample_current_instance,
          n.node_control_version,n.state_drained,
          COALESCE(n.state_applied_boot_id=n.boot_id AND n.state_applied_operation_id IS NOT DISTINCT FROM n.operation_id AND n.state_applied_admin_state=n.admin_state AND n.state_applied_at>s.at-interval '15 seconds' AND n.server_instance=$1,false) state_applied,
          n.agent_version,n.boot_id,n.operation_id,(extract(epoch from n.last_seen_at)*1000)::bigint last_seen,
          (extract(epoch from n.last_seen_at)*1000)::bigint telemetry_sampled_at,
          n.server_instance=$1 AND n.last_seen_at>s.at-($2::bigint*interval '1 second') telemetry_current,
          (extract(epoch from s.at-n.last_seen_at)*1000)::bigint heartbeat_age_ms,
          r.applied_policy_version,r.offered_policy_version,n.connections,n.connections::text connections_exact,
          n.uploaded_bytes,n.downloaded_bytes,n.uploaded_bytes::text uploaded_bytes_exact,n.downloaded_bytes::text downloaded_bytes_exact
          FROM relay_cluster_nodes n CROSS JOIN sample s LEFT JOIN relay_nodes r ON r.node_id=n.node_id WHERE $1::uuid IS NOT NULL"#
    } else {
        r#"SELECT r.node_id id,r.public_url url,NULL::text machine_ip,NULL::text region,NULL::integer weight,NULL::integer capacity_mbps,
          CASE WHEN r.telemetry_boot_id IS NOT NULL THEN 'host' END capacity_mode,
          r.server_instance=$1 AND r.last_seen_at>s.at-($2::bigint*interval '1 second') configured,
          NULL::text admin_state,
          CASE WHEN r.server_instance=$1 AND r.last_seen_at>s.at-($2::bigint*interval '1 second') THEN 'healthy' ELSE 'offline' END health,
          r.server_instance=$1 AND r.last_seen_at>s.at-($2::bigint*interval '1 second') online,
          r.server_instance=$1 sample_current_instance,
          0::integer node_control_version,false state_drained,false state_applied,
          r.agent_version,r.telemetry_boot_id boot_id,NULL::uuid operation_id,(extract(epoch from r.last_seen_at)*1000)::bigint last_seen,
          (extract(epoch from r.telemetry_received_at)*1000)::bigint telemetry_sampled_at,
          t.fresh telemetry_current,
          (extract(epoch from s.at-r.last_seen_at)*1000)::bigint heartbeat_age_ms,
          r.applied_policy_version,r.offered_policy_version,
          CASE WHEN t.fresh THEN r.telemetry_connections END connections,
          CASE WHEN t.fresh THEN r.telemetry_connections::text END connections_exact,
          CASE WHEN t.fresh THEN r.telemetry_received_bytes END uploaded_bytes,
          CASE WHEN t.fresh THEN r.telemetry_sent_bytes END downloaded_bytes,
          CASE WHEN t.fresh THEN r.telemetry_received_bytes::text END uploaded_bytes_exact,
          CASE WHEN t.fresh THEN r.telemetry_sent_bytes::text END downloaded_bytes_exact
          FROM relay_nodes r CROSS JOIN sample s CROSS JOIN LATERAL
            (SELECT COALESCE(r.telemetry_server_instance=$1 AND r.server_instance=$1
              AND r.telemetry_received_at>s.at-($2::bigint*interval '1 second')
              AND r.last_seen_at>s.at-($2::bigint*interval '1 second'),false) fresh) t"#
    };
    let sql = format!(
        r#"WITH sample AS (SELECT statement_timestamp() at), nodes AS ({nodes}),
      classified AS (SELECT *,configured AND online AND COALESCE(admin_state='active',true) AND (node_control_version=0 OR state_applied) available,
        configured AND admin_state='maintenance' maintenance,configured AND admin_state='disabled' disabled,
        configured AND COALESCE(admin_state<>'disabled',true) AND health<>'healthy' abnormal,NOT configured history FROM nodes),
      filtered AS (SELECT * FROM classified WHERE position(lower($3) in lower(id))>0 AND
        CASE $4 WHEN 'configured' THEN configured WHEN 'available' THEN available WHEN 'maintenance' THEN maintenance
          WHEN 'disabled' THEN disabled WHEN 'abnormal' THEN abnormal WHEN 'history' THEN history ELSE true END),
      page_rows AS (SELECT * FROM filtered ORDER BY id LIMIT $5 OFFSET $6)
      SELECT jsonb_build_object('items',COALESCE((SELECT jsonb_agg(to_jsonb(p) ORDER BY id) FROM page_rows p),'[]'::jsonb),
        'total',(SELECT count(*) FROM filtered),'page',$7::bigint,'page_size',$5::bigint,
        'mode',$8::text,'server_instance',$1::uuid,'semantics_version',1,'offline_seconds',$2::bigint,'sampled_at',(SELECT (extract(epoch from at)*1000)::bigint FROM sample),
        'summary',(SELECT jsonb_build_object('configured',count(*) FILTER(WHERE configured),'available',count(*) FILTER(WHERE available),
          'maintenance',count(*) FILTER(WHERE maintenance),'disabled',count(*) FILTER(WHERE disabled),
          'abnormal',count(*) FILTER(WHERE abnormal),'history',count(*) FILTER(WHERE history)) FROM classified))::text"#
    );
    // This read uses a single MVCC snapshot, including filtering, counters and pagination.
    let payload = sqlx::query_scalar::<_, String>(&sql)
        .bind(state.server_instance)
        .bind(offline_seconds as i64)
        .bind(search)
        .bind(query.status.map_or("", RelayStatus::name))
        .bind(size)
        .bind((page - 1) * size)
        .bind(page)
        .bind(if config.is_some() {
            "cluster"
        } else {
            "legacy"
        })
        .fetch_one(state.control.store().pool())
        .await?;
    let mut snapshot: Value = serde_json::from_str(&payload).map_err(|_| {
        WebError::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "server_error",
        )
    })?;
    let database_elapsed = started.elapsed();
    if let Some(items) = snapshot["items"].as_array_mut() {
        let addresses = futures_util::future::join_all(items.iter().map(|item| async {
            if let Some(ip) = item["machine_ip"].as_str() { return vec![ip.to_owned()]; }
            state.relay_addresses.resolve(item["url"].as_str().unwrap_or("")).await
        })).await;
        for (item, addresses) in items.iter_mut().zip(addresses) {
            item["ip_addresses"] = serde_json::json!(addresses);
            item["ip_source"] = serde_json::json!(if item["machine_ip"].is_string() { "configured" } else { "relay_url_dns" });
        }
    }
    snapshot["health"] = serde_json::json!({
        "server":"responding", "database":"reachable", "source":"management_database_query",
        "checked_at":snapshot["sampled_at"],
        "latency_grade":if database_elapsed < std::time::Duration::from_millis(250) { "normal" } else { "slow" }
    });
    Ok(snapshot)
}
