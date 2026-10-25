//! Optional LAN diagnostics on the receiver's existing monotonic clock.
//! Fixed 64-byte probes contain no desktop data. They never grant video credit.
use super::remote_desktop_telemetry::emit;
use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const PERIOD: Duration = Duration::from_millis(50);
const DEADLINE: Duration = Duration::from_millis(200);
const MAGIC: &[u8; 8] = b"PABRDNET";

pub fn parse_peer(value: &str) -> Result<SocketAddr, &'static str> {
    let peer: SocketAddr = value.parse().map_err(|_| "network probe requires a literal private IPv4:port")?;
    if !matches!(peer.ip(), IpAddr::V4(ip) if ip.is_private() || ip.is_loopback()) || peer.port() == 0 {
        return Err("network probe requires a literal private IPv4:port");
    }
    Ok(peer)
}

pub struct Monitor {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}
fn micros(duration: Duration) -> u64 { duration.as_micros().min(u64::MAX as u128) as u64 }
fn wait(at: Instant, stop: &AtomicBool) -> bool {
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now >= at { return true; }
        thread::park_timeout(at-now);
    }
    false
}
impl Monitor {
    pub fn start(peer: SocketAddr, started: Instant, duration: Duration) -> io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        socket.connect(peer)?;
        socket.set_read_timeout(Some(Duration::from_millis(20)))?;
        socket.set_write_timeout(Some(Duration::from_millis(20)))?;
        let local = socket.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let mut monitor = Self { stop: stop.clone(), threads: Vec::new() };
        let end = started + duration;
        emit(serde_json::json!({"event":"network_probe_opened","elapsed_us":micros(started.elapsed()),
            "peer":peer.to_string(),"local":local.to_string(),"period_us":micros(PERIOD),
            "timeout_us":micros(DEADLINE),"clock":"receiver_monotonic"}));
        monitor.threads.push(thread::Builder::new().name("smoke-udp".into()).spawn(move || {
            let token = *uuid::Uuid::new_v4().as_bytes();
            let mut sequence = 0u64;
            let mut at = Instant::now();
            while at < end && wait(at, &stop) {
                let sent = Instant::now();
                sequence += 1;
                let mut request = [0u8; 64];
                request[..8].copy_from_slice(MAGIC);
                request[8..16].copy_from_slice(&sequence.to_le_bytes());
                request[16..32].copy_from_slice(&token);
                let mut status = "timeout";
                let mut rtt = None;
                let mut server = None;
                if socket.send(&request).is_err() { status = "send_error"; }
                else {
                    let mut received = [0u8; 65];
                    let mut discarded = 0;
                    while sent.elapsed() < DEADLINE && !stop.load(Ordering::Relaxed) {
                        match socket.recv(&mut received) {
                            Ok(64) if received[..32] == request[..32] => {
                                rtt = Some(micros(sent.elapsed()));
                                server = Some(u64::from_le_bytes(received[32..40].try_into().unwrap()));
                                status = "ok";
                                break;
                            }
                            Ok(_) => { discarded += 1; if discarded == 16 { status = "invalid_reply"; break; } }
                            Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => {}
                            Err(_) => { status = "receive_error"; break; }
                        }
                    }
                }
                emit(serde_json::json!({"event":"network_probe","sequence":sequence,
                    "send_us":micros(sent.duration_since(started)),"elapsed_us":micros(started.elapsed()),
                    "schedule_late_us":micros(sent.saturating_duration_since(at)),
                    "status":status,"rtt_us":rtt,"server_work_us":server}));
                // Never compensate for a timeout with a burst of queued probes.
                at = (at+PERIOD).max(Instant::now()+Duration::from_millis(1));
            }
        })?);
        let stop = monitor.stop.clone();
        monitor.threads.push(thread::Builder::new().name("smoke-schedule".into()).spawn(move || {
            let mut at = Instant::now()+PERIOD;
            while at < end && wait(at, &stop) {
                let now = Instant::now();
                emit(serde_json::json!({"event":"schedule_probe","elapsed_us":micros(now.duration_since(started)),
                    "scheduled_us":micros(at.duration_since(started)),"late_us":micros(now.saturating_duration_since(at))}));
                at = (at+PERIOD).max(now+Duration::from_millis(1));
            }
        })?);
        Ok(monitor)
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for handle in &self.threads { handle.thread().unpark(); }
        for handle in self.threads.drain(..) { let _ = handle.join(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn probe_endpoint_is_explicit_and_lan_only() {
        for value in ["example.com:42", "8.8.8.8:42", "0.0.0.0:42", "10.0.0.1:0", "[::1]:42"] {
            assert!(parse_peer(value).is_err(), "{value}");
        }
        assert!(parse_peer("10.0.0.1:42").is_ok());
        assert!(parse_peer("127.0.0.1:42").is_ok());
    }
}
