//! Legacy process telemetry travels with the existing policy request, so old
//! peers remain compatible and a reporting failure cannot change admission.
use pab_protocol::RelayNodeTelemetry;
use time::OffsetDateTime;

use super::ControlApiState;

pub(super) struct LegacyReporter {
    connected_at: OffsetDateTime,
    identity: Option<(String, uuid::Uuid)>,
}

impl LegacyReporter {
    pub fn new() -> Self {
        Self {
            connected_at: OffsetDateTime::now_utc(),
            identity: None,
        }
    }

    pub fn validate(&mut self, node: Option<&str>, sample: &RelayNodeTelemetry) -> bool {
        let Some(node) = node.filter(|node| pab_protocol::valid_relay_node_id(node)) else {
            return false;
        };
        if sample.boot_id.is_nil()
            || sample.public_url.as_deref().is_some_and(|value| {
                value.len() > 2048
                    || !url::Url::parse(value).is_ok_and(|url| {
                        url.scheme() == "https"
                            && url.host_str().is_some()
                            && url.username().is_empty()
                            && url.password().is_none()
                            && url.query().is_none()
                            && url.fragment().is_none()
                    })
            })
        {
            return false;
        }
        match &self.identity {
            Some((bound_node, boot)) => bound_node == node && *boot == sample.boot_id,
            None => {
                self.identity = Some((node.to_owned(), sample.boot_id));
                true
            }
        }
    }

    pub async fn record(&self, state: &ControlApiState, node: &str, sample: &RelayNodeTelemetry) {
        // A newer control connection takes ownership; a late packet from the old
        // connection must not restore its counters or process identity.
        let result = sqlx::query(
            "UPDATE relay_nodes SET public_url=$2,telemetry_boot_id=$3,telemetry_server_instance=$4,
             telemetry_session_started_at=$5,telemetry_received_at=clock_timestamp(),
             telemetry_connections=$6::text::numeric,telemetry_received_bytes=$7::text::numeric,
             telemetry_sent_bytes=$8::text::numeric WHERE node_id=$1 AND
             (telemetry_server_instance IS DISTINCT FROM $4 OR telemetry_session_started_at IS NULL
              OR telemetry_session_started_at<=$5)",
        )
        .bind(node).bind(&sample.public_url).bind(sample.boot_id).bind(state.server_instance)
        .bind(self.connected_at).bind(sample.connections.to_string())
        .bind(sample.received_bytes.to_string()).bind(sample.sent_bytes.to_string())
        .execute(state.control.store().pool()).await;
        if result.is_err() {
            tracing::warn!(node, "could not record Relay process telemetry");
        } else if result.is_ok_and(|r|r.rows_affected()>0) {
            if let Some(diagnostics)=&sample.diagnostics {
                if !state.relay_diagnostics.record(node,sample.boot_id,diagnostics.clone()){tracing::warn!(node,"Relay diagnostics unavailable");}
            }
        }
    }
}
