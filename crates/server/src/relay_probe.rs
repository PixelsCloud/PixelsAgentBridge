//! One short-lived management probe per configured node. Never registers a device.
use pab_protocol::{EndpointKey, RelayProbeGrant, RelayProbeSnapshot};
use std::{
    collections::HashMap,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Debug)]
struct Probe {
    boot: uuid::Uuid,
    grant: RelayProbeGrant,
    deadline: Instant,
    applied: bool,
    url: String,
}
#[derive(Debug, Default)]
pub(crate) struct RelayProbes {
    nodes: HashMap<String, Probe>,
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
impl RelayProbes {
    pub fn create(
        &mut self,
        node: &str,
        boot: uuid::Uuid,
        first: EndpointKey,
        second: EndpointKey,
        url: String,
    ) -> Result<uuid::Uuid, &'static str> {
        self.nodes.retain(|_, p| p.deadline > Instant::now());
        if self.nodes.contains_key(node) {
            return Err("a Relay probe is already pending");
        }
        if self.nodes.len() >= 64 {
            return Err("too many Relay probes");
        }
        let probe_id = uuid::Uuid::new_v4();
        self.nodes.insert(
            node.into(),
            Probe {
                boot,
                grant: RelayProbeGrant {
                    probe_id,
                    first,
                    second,
                    expires_at_unix_ms: now_ms() + 10_000,
                    max_bytes: 65_536,
                },
                deadline: Instant::now() + Duration::from_secs(10),
                applied: false,
                url,
            },
        );
        Ok(probe_id)
    }
    pub fn snapshot(
        &mut self,
        node: &str,
        boot: uuid::Uuid,
        applied: Option<uuid::Uuid>,
    ) -> RelayProbeSnapshot {
        let grant = self
            .nodes
            .get_mut(node)
            .filter(|p| p.boot == boot && p.deadline > Instant::now())
            .map(|p| {
                if applied == Some(p.grant.probe_id) {
                    p.applied = true;
                }
                p.grant.clone()
            });
        RelayProbeSnapshot {
            node_id: node.into(),
            boot_id: boot,
            issued_at_unix_ms: now_ms(),
            grant,
        }
    }
    pub fn status(&self, id: uuid::Uuid) -> Option<serde_json::Value> {
        self.nodes.iter().find(|(_,p)|p.grant.probe_id==id).map(|(node,p)|serde_json::json!({"probe_id":id,"node_id":node,
            "url":p.url,"applied":p.applied,"expired":p.deadline<=Instant::now(),"expires_at_unix_ms":p.grant.expires_at_unix_ms}))
    }
    pub fn remove(&mut self, id: uuid::Uuid) -> bool {
        let before = self.nodes.len();
        self.nodes.retain(|_, p| p.grant.probe_id != id);
        self.nodes.len() != before
    }
}
