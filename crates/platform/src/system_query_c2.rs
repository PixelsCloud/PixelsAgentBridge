use super::system_query::{bound_reply, bounded, failed, now, push_bounded};
use hickory_resolver::{
    TokioResolver,
    config::ResolveHosts,
    proto::rr::{Name, RecordType},
};
use pab_protocol::*;
use std::time::{Duration, Instant};

pub(super) fn connections(
    mut r: SystemQueryReply,
    filter: &ConnectionFilter,
    limit: u16,
) -> SystemQueryReply {
    use netstat2::{AddressFamilyFlags as A, ProtocolFlags as P};
    r.sampled_from_unix_ms = Some(now());
    let start = Instant::now();
    let families = match filter.family {
        Some(IpFamily::Ipv4) => A::IPV4,
        Some(IpFamily::Ipv6) => A::IPV6,
        None => A::all(),
    };
    let protocols = match filter.protocol {
        Some(SocketProtocol::Tcp) => P::TCP,
        Some(SocketProtocol::Udp) => P::UDP,
        None => P::all(),
    };
    let sockets = match netstat2::iterate_sockets_info(families, protocols) {
        Ok(v) => v,
        Err(e) => {
            return failed(
                r,
                &format!(
                    "socket inventory unavailable: {}",
                    bounded(&e.to_string(), 512)
                ),
            );
        }
    };
    let mut entries = vec![];
    let mut bytes = 0;
    for (scanned, item) in sockets.enumerate() {
        if scanned >= 100_000 || start.elapsed() > Duration::from_secs(5) {
            r.truncated = true;
            r.stop_reason = Some(
                if scanned >= 100_000 {
                    "scan_entry_limit"
                } else {
                    "collection_elapsed_limit"
                }
                .into(),
            );
            break;
        }
        let socket = match item {
            Ok(s) => s,
            Err(e) => {
                r.truncated = true;
                r.stop_reason = Some("backend_error".into());
                if r.warnings.len() < 6 {
                    r.warnings.push(bounded(&e.to_string(), 256));
                }
                continue;
            }
        };
        let mut c = connection_info(socket);
        if !filter.matches(&c) {
            continue;
        }
        if entries.len() >= limit as usize {
            r.truncated = true;
            r.stop_reason = Some("entry_limit".into());
            break;
        }
        c.pids_truncated = c.pids.len() > 64;
        c.pids.truncate(64);
        if !push_bounded(&mut entries, c, &mut bytes, &mut r) {
            break;
        }
    }
    r.data = Some(SystemQueryData::Connections {
        backend: "netstat2".into(),
        entries,
    });
    r.state = "completed".into();
    r.sampled_at_unix_ms = Some(now());
    r.warnings.push("Socket inventory is not atomic and is limited to this process's OS/network namespace and permissions. Empty pids means unknown; UDP peer/state are not available. TCP listening peers are null.".into());
    bound_reply(&mut r);
    r
}
fn connection_info(socket: netstat2::SocketInfo) -> ConnectionInfo {
    use netstat2::{ProtocolSocketInfo as P, TcpState as T};
    let (protocol, local_address, local_port, remote_address, remote_port, state) =
        match socket.protocol_socket_info {
            P::Tcp(s) => {
                let state = match s.state {
                    T::Closed => "closed",
                    T::Listen => "listen",
                    T::SynSent => "syn_sent",
                    T::SynReceived => "syn_received",
                    T::Established => "established",
                    T::FinWait1 => "fin_wait_1",
                    T::FinWait2 => "fin_wait_2",
                    T::CloseWait => "close_wait",
                    T::Closing => "closing",
                    T::LastAck => "last_ack",
                    T::TimeWait => "time_wait",
                    T::DeleteTcb => "delete_tcb",
                    T::Unknown => "unknown",
                };
                let peer = s.state != T::Listen && s.remote_port != 0;
                (
                    SocketProtocol::Tcp,
                    s.local_addr,
                    s.local_port,
                    peer.then_some(s.remote_addr),
                    peer.then_some(s.remote_port),
                    Some(state.into()),
                )
            }
            P::Udp(s) => (
                SocketProtocol::Udp,
                s.local_addr,
                s.local_port,
                None,
                None,
                None,
            ),
        };
    let mut pids = socket.associated_pids;
    pids.sort_unstable();
    pids.dedup();
    ConnectionInfo {
        protocol,
        family: if local_address.is_ipv4() {
            IpFamily::Ipv4
        } else {
            IpFamily::Ipv6
        },
        local_address,
        local_port,
        remote_address,
        remote_port,
        state,
        pids,
        pids_truncated: false,
    }
}

/// Async DNS and session backends use the same durable query contract as C1.
pub async fn query_async(id: RequestId, q: &SystemQuery) -> Option<SystemQueryReply> {
    if let Some(reply) = super::system_query_c3::query_async(id, q).await {
        return Some(reply);
    }
    match q {
        SystemQuery::Dns {
            name,
            record_type,
            timeout_ms,
            limit,
        } => {
            let mut r = SystemQueryReply::pending(id, q);
            if let Err(e) = q.validate() {
                return Some(failed(r, e));
            }
            r.sampled_from_unix_ms = Some(now());
            let loaded = tokio::task::spawn_blocking(TokioResolver::builder_tokio)
                .await
                .map_err(|e| e.to_string())
                .and_then(|b| b.map_err(|e| e.to_string()));
            let builder = match loaded {
                Ok(b) => b,
                Err(e) => {
                    return Some(failed(
                        r,
                        &format!("system DNS configuration unavailable: {}", bounded(&e, 512)),
                    ));
                }
            };
            Some(dns_with_builder(r, name, *record_type, *timeout_ms, *limit, builder).await)
        }
        SystemQuery::Sessions { user, state, limit } => {
            let mut r = SystemQueryReply::pending(id, q);
            if let Err(e) = q.validate() {
                return Some(failed(r, e));
            }
            r.sampled_from_unix_ms = Some(now());
            match pab_os_sessions::collect().await {
                Ok(batch) => Some(session_reply(
                    r,
                    batch,
                    user.as_deref(),
                    state.as_deref(),
                    *limit,
                )),
                Err(e) => Some(failed(r, &bounded(&e, 1024))),
            }
        }
        _ => None,
    }
}
async fn dns_with_builder(
    mut r: SystemQueryReply,
    name: &str,
    kind: DnsRecordType,
    timeout_ms: u32,
    limit: u16,
    mut builder: hickory_resolver::ResolverBuilder<
        hickory_resolver::net::runtime::TokioRuntimeProvider,
    >,
) -> SystemQueryReply {
    let options = builder.options_mut();
    options.timeout = Duration::from_millis(timeout_ms as u64);
    options.attempts = 1;
    options.cache_size = 0;
    options.use_hosts_file = ResolveHosts::Never;
    options.preserve_intermediates = false;
    let resolver = match builder.build() {
        Ok(v) => v,
        Err(e) => {
            return failed(
                r,
                &format!("DNS resolver unavailable: {}", bounded(&e.to_string(), 512)),
            );
        }
    };
    let query_name = if kind == DnsRecordType::Ptr {
        name.parse::<std::net::IpAddr>()
            .ok()
            .map(Name::from)
            .map(|n| n.to_string())
            .unwrap_or_else(|| name.into())
    } else {
        name.into()
    };
    let record_type = match kind {
        DnsRecordType::A => RecordType::A,
        DnsRecordType::Aaaa => RecordType::AAAA,
        DnsRecordType::Cname => RecordType::CNAME,
        DnsRecordType::Mx => RecordType::MX,
        DnsRecordType::Ns => RecordType::NS,
        DnsRecordType::Ptr => RecordType::PTR,
        DnsRecordType::Soa => RecordType::SOA,
        DnsRecordType::Srv => RecordType::SRV,
        DnsRecordType::Txt => RecordType::TXT,
    };
    let lookup = match tokio::time::timeout(
        Duration::from_millis(timeout_ms as u64),
        resolver.lookup(query_name.as_str(), record_type),
    )
    .await
    {
        Err(_) => return failed(r, "DNS lookup timed out"),
        Ok(Err(e)) => {
            return failed(
                r,
                &format!("DNS lookup failed: {}", bounded(&e.to_string(), 512)),
            );
        }
        Ok(Ok(v)) => v,
    };
    let mut records = vec![];
    let mut bytes = 0;
    for record in lookup.answers() {
        if records.len() >= limit as usize {
            r.truncated = true;
            r.stop_reason = Some("entry_limit".into());
            break;
        }
        let value = record.data.to_string();
        let value_truncated = value.len() > 4096;
        let item = DnsRecord {
            name: bounded(&record.name.to_string(), 512),
            record_type: record.record_type().to_string(),
            ttl_seconds: record.ttl,
            value: bounded(&value, 4096),
            value_truncated,
        };
        if !push_bounded(&mut records, item, &mut bytes, &mut r) {
            break;
        }
    }
    r.data = Some(SystemQueryData::Dns {
        result: DnsResult {
            requested_name: name.into(),
            query_name,
            record_type: kind,
            resolver: "hickory_system_dns_configuration".into(),
            hosts_file_consulted: false,
            records,
        },
    });
    r.state = "completed".into();
    r.sampled_at_unix_ms = Some(now());
    r.warnings.push("Uses target system DNS configuration through Hickory, not the native OS resolver. Does not consult hosts, mDNS or Windows NRPT/VPN split-DNS policies. Short names may use configured search domains. Cache is disabled; upstream resolvers may still cache.".into());
    bound_reply(&mut r);
    r
}

fn session_reply(
    mut r: SystemQueryReply,
    batch: pab_os_sessions::SessionBatch,
    user: Option<&str>,
    state: Option<&str>,
    limit: u16,
) -> SystemQueryReply {
    r.truncated = batch.truncated;
    if batch.truncated {
        r.stop_reason = Some("backend_entry_limit".into());
    }
    let mut entries = vec![];
    let mut bytes = 0;
    for s in batch.entries {
        if user.is_some_and(|u| s.user_name.as_deref() != Some(u))
            || state.is_some_and(|v| v != s.state)
        {
            continue;
        }
        if entries.len() >= limit as usize {
            r.truncated = true;
            r.stop_reason = Some("entry_limit".into());
            break;
        }
        if !push_bounded(&mut entries, s, &mut bytes, &mut r) {
            break;
        }
    }
    r.data = Some(SystemQueryData::Sessions {
        backend: batch.backend.into(),
        entries,
    });
    r.state = "completed".into();
    r.sampled_at_unix_ms = Some(now());
    r.warnings.push("OS login sessions, not MCP/terminal sessions or user accounts. Inventory is not atomic; null/errors indicate inaccessible or disappeared fields. Windows may include service/listener sessions without a logged-in user.".into());
    bound_reply(&mut r);
    r
}

#[cfg(test)]
#[path = "system_query_c2_tests.rs"]
mod tests;
