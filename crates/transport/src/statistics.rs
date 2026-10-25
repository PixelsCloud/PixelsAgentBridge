use iroh::endpoint::Connection;
use serde::Serialize;

/// Sender loss detection counters; these are not receiver loss or exact retransmits.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionStatistics {
    pub path_id: String,
    pub remote_addr: String,
    pub local_addr: String,
    pub rtt_us: u64,
    pub lost_packets: u64,
    pub lost_bytes: u64,
    pub tx_datagrams: u64,
    pub rx_datagrams: u64,
    pub congestion_events: u64,
    pub cwnd: u64,
    pub stream_frames_tx: u64,
}
pub(crate) fn snapshot(connection: &Connection) -> Option<ConnectionStatistics> {
    if connection.close_reason().is_some() { return None; }
    let paths = connection.paths();
    let path = paths.iter().find(|p| p.is_selected())?;
    let s = path.stats();
    Some(ConnectionStatistics {
        path_id: path.id().to_string(),
        remote_addr: path.remote_addr().to_string(),
        local_addr: format!("{:?}", path.local_addr()),
        rtt_us: s.rtt.as_micros().min(u64::MAX as u128) as u64,
        lost_packets: s.lost_packets, lost_bytes: s.lost_bytes,
        tx_datagrams: s.udp_tx.datagrams, rx_datagrams: s.udp_rx.datagrams,
        congestion_events: s.congestion_events, cwnd: s.cwnd,
        stream_frames_tx: s.frame_tx.stream,
    })
}
