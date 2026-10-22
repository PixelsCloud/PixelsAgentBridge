//! Live observations are bounded and refreshed from Relay's complete snapshots.
//! They do not modify application identities, accounting or administrative state.
use pab_protocol::RelayDiagnosticsSnapshot;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Debug, Clone)]
pub(crate) struct Sample {
    pub boot: uuid::Uuid,
    pub received_at_ms: i64,
    pub received: Instant,
    pub snapshot: Arc<RelayDiagnosticsSnapshot>,
}
#[derive(Debug, Default)]
pub(crate) struct RelayDiagnostics {
    samples: Mutex<HashMap<String, Sample>>,
}
impl RelayDiagnostics {
    pub fn record(&self, node: &str, boot: uuid::Uuid, snapshot: RelayDiagnosticsSnapshot) -> bool {
        if !snapshot.valid() {
            return false;
        }
        let Ok(mut samples) = self.samples.lock() else {
            return false;
        };
        if samples.get(node).is_some_and(|old| {
            old.boot == boot
                && old.snapshot.host.process_uptime_ms >= snapshot.host.process_uptime_ms
        }) {
            return true;
        }
        if samples.len() >= pab_protocol::MAX_RELAY_NODES && !samples.contains_key(node) {
            if let Some(old) = samples
                .iter()
                .min_by_key(|(_, v)| v.received)
                .map(|(k, _)| k.clone())
            {
                samples.remove(&old);
            }
        }
        samples.insert(
            node.to_owned(),
            Sample {
                boot,
                received_at_ms: (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
                    as i64,
                received: Instant::now(),
                snapshot: Arc::new(snapshot),
            },
        );
        true
    }
    pub fn get(&self, node: &str) -> Option<Sample> {
        self.samples.lock().ok()?.get(node).cloned()
    }
}
