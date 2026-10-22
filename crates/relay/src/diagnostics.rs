use iroh_base::EndpointId;
use iroh_relay::server::ConnectionId;
use pab_protocol::{
    EndpointKey, RELAY_DIAGNOSTICS_CONNECTION_LIMIT, RELAY_DIAGNOSTICS_LINK_LIMIT,
    RELAY_DIAGNOSTICS_VERSION, RELAY_LINK_ACTIVE_MS, RelayConnectionSnapshot,
    RelayDiagnosticsSnapshot, RelayHostSnapshot, RelayLinkSnapshot,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
    time::{Duration, Instant},
};

const RETAIN: Duration = Duration::from_secs(20 * 60);
const TRACK_LIMIT: usize = 8192;
type Pair = ([u8; 32], [u8; 32]);
#[derive(Debug)]
struct Connection {
    id: ConnectionId,
    endpoint: EndpointId,
    probe: bool,
    start: Instant,
    end: Option<Instant>,
}
#[derive(Debug)]
struct Link {
    id: uuid::Uuid,
    start: Instant,
    last: Instant,
    end: Option<Instant>,
    bytes: [u64; 2],
    packets: [u64; 2],
    previous: [u64; 2],
}
#[derive(Debug)]
struct Tracker {
    connections: HashMap<ConnectionId, Connection>,
    closed: VecDeque<Connection>,
    links: HashMap<Pair, Link>,
    last_sample: Instant,
    previous_bytes: [u64; 2],
    limited: bool,
}
#[derive(Debug)]
pub(crate) struct Diagnostics {
    start: Instant,
    tracker: Mutex<Tracker>,
    system: Mutex<sysinfo::System>,
}
fn ms(value: Duration) -> u64 {
    value.as_millis().min(u64::MAX as u128) as u64
}
fn clean(value: String) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(128)
        .collect()
}
impl Default for Diagnostics {
    fn default() -> Self {
        let start = Instant::now();
        let mut system = sysinfo::System::new();
        system.refresh_cpu_all();
        Self {
            start,
            tracker: Mutex::new(Tracker {
                connections: HashMap::new(),
                closed: VecDeque::new(),
                links: HashMap::new(),
                last_sample: start,
                previous_bytes: [0; 2],
                limited: false,
            }),
            system: Mutex::new(system),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(key: EndpointId) -> iroh_relay::server::ClientRequest {
        iroh_relay::server::ClientRequest::new(
            key,
            iroh_relay::http::ProtocolVersion::V2,
            tokio_tungstenite::tungstenite::http::Request::new(())
                .into_parts()
                .0,
        )
    }
    #[test]
    fn observes_bidirectional_pairs_and_duplicate_connection_lifetimes() {
        let tracker = Diagnostics::default();
        let metrics = iroh_relay::server::Metrics::default();
        let a = iroh_base::SecretKey::generate().public();
        let b = iroh_base::SecretKey::generate().public();
        let first = request(a);
        let duplicate = request(a);
        let peer = request(b);
        for row in [&first, &duplicate, &peer] {
            tracker.connected(row.endpoint_id(), row.connection_id(), false);
            metrics.accepts.inc();
        }
        tracker.forwarded(a, b, 123);
        tracker.forwarded(b, a, 456);
        let sample = tracker.snapshot(&metrics).unwrap();
        assert!(sample.valid());
        assert_eq!(sample.connections_total, 3);
        assert_eq!(sample.connections.len(), 3);
        assert_eq!(sample.links.len(), 1);
        assert_eq!(sample.active_links, 1);
        let link = &sample.links[0];
        assert_eq!(link.a_to_b_bytes + link.b_to_a_bytes, 579);
        assert_eq!(link.a_to_b_packets + link.b_to_a_packets, 2);
        tracker.disconnected(a, first.connection_id());
        assert!(
            tracker.snapshot(&metrics).unwrap().links[0]
                .closed_ago_ms
                .is_none()
        );
        tracker.disconnected(a, duplicate.connection_id());
        let sample = tracker.snapshot(&metrics).unwrap();
        assert!(sample.links[0].closed_ago_ms.is_some());
        assert_eq!(sample.active_links, 0);
        assert_eq!(
            sample
                .connections
                .iter()
                .filter(|r| r.closed_ago_ms.is_some())
                .count(),
            2
        );
        let old = sample.links[0].id;
        tracker.forwarded(a, b, 1);
        assert!(
            tracker.snapshot(&metrics).unwrap().links[0]
                .closed_ago_ms
                .is_some()
        );
        let new = request(a);
        tracker.connected(a, new.connection_id(), false);
        tracker.forwarded(a, b, 1);
        let sample = tracker.snapshot(&metrics).unwrap();
        assert_ne!(sample.links[0].id, old);
        assert_eq!(
            sample.links[0].a_to_b_bytes + sample.links[0].b_to_a_bytes,
            1
        );
    }
    #[test]
    fn idle_links_stop_counting_as_active_and_inventory_stays_bounded() {
        let tracker = Diagnostics::default();
        let metrics = iroh_relay::server::Metrics::default();
        let a = iroh_base::SecretKey::generate().public();
        let b = iroh_base::SecretKey::generate().public();
        tracker.forwarded(a, b, 100);
        {
            let mut state = tracker.tracker.lock().unwrap();
            let link = state.links.values_mut().next().unwrap();
            link.last = Instant::now() - Duration::from_secs(16);
            link.start = link.last - Duration::from_secs(2);
        }
        let sample = tracker.snapshot(&metrics).unwrap();
        assert_eq!(sample.active_links, 0);
        assert!(sample.links[0].closed_ago_ms.is_none());
        assert!(sample.valid());
        for _ in 0..RELAY_DIAGNOSTICS_CONNECTION_LIMIT + 10 {
            let row = request(a);
            tracker.connected(a, row.connection_id(), false);
        }
        let sample = tracker.snapshot(&metrics).unwrap();
        assert_eq!(sample.connections.len(), RELAY_DIAGNOSTICS_CONNECTION_LIMIT);
        assert!(sample.connections_truncated);
        assert!(sample.valid());
        assert!(serde_json::to_vec(&sample).unwrap().len() < 2_000_000);
    }
}
impl Diagnostics {
    pub(crate) fn connected(&self, endpoint: EndpointId, id: ConnectionId, probe: bool) {
        let Ok(mut state) = self.tracker.lock() else {
            return;
        };
        if state.connections.len() >= TRACK_LIMIT {
            state.limited = true;
            return;
        }
        state.connections.insert(
            id,
            Connection {
                id,
                endpoint,
                probe,
                start: Instant::now(),
                end: None,
            },
        );
    }
    pub(crate) fn disconnected(&self, endpoint: EndpointId, id: ConnectionId) {
        let Ok(mut state) = self.tracker.lock() else {
            return;
        };
        let now = Instant::now();
        if let Some(mut row) = state.connections.remove(&id) {
            row.end = Some(now);
            state.closed.push_back(row);
            if state.closed.len() > 128 {
                state.closed.pop_front();
            }
        }
        if !state
            .connections
            .values()
            .any(|row| row.endpoint == endpoint)
        {
            for (key, row) in &mut state.links {
                if (key.0 == *endpoint.as_bytes() || key.1 == *endpoint.as_bytes())
                    && row.end.is_none()
                {
                    row.end = Some(now);
                }
            }
        }
    }
    pub(crate) fn forwarded(&self, src: EndpointId, dst: EndpointId, bytes: usize) {
        if src == dst {
            return;
        }
        let now = Instant::now();
        let (a, b, direction) = if src.as_bytes() < dst.as_bytes() {
            (*src.as_bytes(), *dst.as_bytes(), 0)
        } else {
            (*dst.as_bytes(), *src.as_bytes(), 1)
        };
        let Ok(mut state) = self.tracker.lock() else {
            return;
        };
        if !state.links.contains_key(&(a, b)) && state.links.len() >= TRACK_LIMIT {
            state
                .links
                .retain(|_, v| now.duration_since(v.last) < RETAIN);
            if state.links.len() >= TRACK_LIMIT {
                state.limited = true;
                return;
            }
        }
        let both_connected = state.links.get(&(a,b)).is_some_and(|r|r.end.is_some())
            && state.connections.values().any(|r| r.endpoint == src)
            && state.connections.values().any(|r| r.endpoint == dst);
        let row = state.links.entry((a, b)).or_insert_with(|| Link {
            id: uuid::Uuid::new_v4(),
            start: now,
            last: now,
            end: None,
            bytes: [0; 2],
            packets: [0; 2],
            previous: [0; 2],
        });
        if row.end.is_some() && both_connected {
            *row = Link {
                id: uuid::Uuid::new_v4(),
                start: now,
                last: now,
                end: None,
                bytes: [0; 2],
                packets: [0; 2],
                previous: [0; 2],
            };
        }
        row.last = now;
        row.bytes[direction] = row.bytes[direction].saturating_add(bytes as u64);
        row.packets[direction] = row.packets[direction].saturating_add(1);
    }
    pub(crate) fn snapshot(
        &self,
        metrics: &iroh_relay::server::Metrics,
    ) -> Option<RelayDiagnosticsSnapshot> {
        let now = Instant::now();
        let mut traffic = pab_protocol::RelayTrafficSnapshot {
            accepted_connections: metrics.accepts.get(),
            disconnected_connections: metrics.disconnects.get(),
            received_bytes: metrics.bytes_recv.get(),
            sent_bytes: metrics.bytes_sent.get(),
            received_packets: metrics.send_packets_recv.get(),
            sent_packets: metrics.send_packets_sent.get(),
            dropped_packets: metrics.send_packets_dropped.get(),
            rate_limited_bytes: metrics.bytes_rx_ratelimited_total.get(),
            ping_frames: metrics.got_ping.get(),
            ..Default::default()
        };
        let (connections, links, links_total, active_links, connections_truncated, links_truncated) = {
            let mut state = self.tracker.lock().ok()?;
            state
                .closed
                .retain(|r| r.end.is_some_and(|end| now.duration_since(end) < RETAIN));
            state
                .links
                .retain(|_, r| now.duration_since(r.last) < RETAIN);
            let mut rows: Vec<_> = state
                .connections
                .values()
                .chain(state.closed.iter())
                .collect();
            rows.sort_by_key(|r| (r.end.is_some(), std::cmp::Reverse(r.start)));
            let connections_truncated =
                rows.len() > RELAY_DIAGNOSTICS_CONNECTION_LIMIT || state.limited;
            let connections = rows
                .into_iter()
                .take(RELAY_DIAGNOSTICS_CONNECTION_LIMIT)
                .map(|r| RelayConnectionSnapshot {
                    id: r.id.to_string(),
                    endpoint: EndpointKey::new(*r.endpoint.as_bytes()),
                    probe: r.probe,
                    age_ms: ms(now.duration_since(r.start)),
                    connected_ms: ms(r.end.unwrap_or(now).duration_since(r.start)),
                    closed_ago_ms: r.end.map(|end| ms(now.duration_since(end))),
                })
                .collect();
            let window = now
                .duration_since(state.last_sample)
                .as_secs_f64()
                .max(0.001);
            state.last_sample = now;
            traffic.received_bytes_per_second = (traffic
                .received_bytes
                .saturating_sub(state.previous_bytes[0])
                as f64
                / window) as u64;
            traffic.sent_bytes_per_second =
                (traffic.sent_bytes.saturating_sub(state.previous_bytes[1]) as f64 / window) as u64;
            state.previous_bytes = [traffic.received_bytes, traffic.sent_bytes];
            let links_total = state.links.len() as u64;
            let active_links = state
                .links
                .values()
                .filter(|r| {
                    r.end.is_none() && ms(now.duration_since(r.last)) <= RELAY_LINK_ACTIVE_MS
                })
                .count() as u64;
            let limited = state.limited;
            let mut rows: Vec<_> = state.links.iter_mut().collect();
            rows.sort_by_key(|(_, r)| (r.end.is_some(), std::cmp::Reverse(r.last)));
            let links_truncated = rows.len() > RELAY_DIAGNOSTICS_LINK_LIMIT || limited;
            let links = rows
                .into_iter()
                .enumerate()
                .filter_map(|(index, ((a, b), r))| {
                    let rates = [
                        ((r.bytes[0] - r.previous[0]) as f64 / window) as u64,
                        ((r.bytes[1] - r.previous[1]) as f64 / window) as u64,
                    ];
                    r.previous = r.bytes;
                    (index < RELAY_DIAGNOSTICS_LINK_LIMIT).then(|| RelayLinkSnapshot {
                        id: r.id,
                        a: EndpointKey::new(*a),
                        b: EndpointKey::new(*b),
                        age_ms: ms(now.duration_since(r.start)),
                        observed_ms: ms(r.end.unwrap_or(now).duration_since(r.start)),
                        last_forwarded_ago_ms: ms(now.duration_since(r.last)),
                        closed_ago_ms: r.end.map(|end| ms(now.duration_since(end))),
                        a_to_b_bytes: r.bytes[0],
                        b_to_a_bytes: r.bytes[1],
                        a_to_b_packets: r.packets[0],
                        b_to_a_packets: r.packets[1],
                        a_to_b_bytes_per_second: rates[0],
                        b_to_a_bytes_per_second: rates[1],
                    })
                })
                .collect();
            (
                connections,
                links,
                links_total,
                active_links,
                connections_truncated,
                links_truncated,
            )
        };
        let host = {
            let mut system = self.system.lock().ok()?;
            system.refresh_cpu_usage();
            system.refresh_memory();
            let pid = sysinfo::Pid::from_u32(std::process::id());
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&[pid]),
                true,
                sysinfo::ProcessRefreshKind::nothing().with_memory(),
            );
            RelayHostSnapshot {
                hostname: sysinfo::System::host_name().map(clean),
                os: std::env::consts::OS.into(),
                os_version: sysinfo::System::long_os_version().map(clean),
                architecture: std::env::consts::ARCH.into(),
                cpu_model: system
                    .cpus()
                    .first()
                    .map(|cpu| clean(cpu.brand().to_owned())),
                logical_cpus: system.cpus().len() as u32,
                cpu_usage_basis_points: (now.duration_since(self.start)
                    > sysinfo::MINIMUM_CPU_UPDATE_INTERVAL)
                    .then(|| (system.global_cpu_usage().clamp(0., 100.) * 100.).round() as u16),
                memory_total: system.total_memory(),
                memory_used: system.used_memory().min(system.total_memory()),
                host_uptime_ms: sysinfo::System::uptime().saturating_mul(1000),
                process_uptime_ms: ms(now.duration_since(self.start)),
                process_memory: system.process(pid).map(|p| p.memory()),
                containerized: std::path::Path::new("/.dockerenv").exists()
                    || std::path::Path::new("/run/.containerenv").exists(),
            }
        };
        let connections_total = metrics
            .accepts
            .get()
            .saturating_sub(metrics.disconnects.get());
        Some(RelayDiagnosticsSnapshot {
            version: RELAY_DIAGNOSTICS_VERSION,
            host,
            traffic,
            connections_total,
            links_total,
            active_links,
            connections_truncated,
            links_truncated,
            connections,
            links,
        })
    }
}
