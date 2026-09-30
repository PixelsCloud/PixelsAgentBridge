use super::*;
use hickory_resolver::{
    config::{NameServerConfig, ResolverConfig},
    net::runtime::TokioRuntimeProvider,
    proto::{
        op::{Message, ResponseCode},
        rr::{RData, Record, rdata::*},
    },
};
use std::net::{TcpListener, TcpStream, UdpSocket};
fn sockets(filter: ConnectionFilter) -> SystemQueryReply {
    super::super::SystemCollector::default().query(
        RequestId::new(),
        &SystemQuery::Connections { filter, limit: 100 },
    )
}
fn entries(r: &SystemQueryReply) -> &[ConnectionInfo] {
    assert_eq!(r.state, "completed", "{r:?}");
    let Some(SystemQueryData::Connections { entries, .. }) = &r.data else {
        panic!()
    };
    entries
}
#[test]
fn native_tcp_ipv4_ipv6_listeners_established_and_close_are_observed() {
    for addr in ["127.0.0.1:0", "[::1]:0"] {
        let listener = TcpListener::bind(addr).unwrap();
        let address = listener.local_addr().unwrap();
        let filter = ConnectionFilter {
            protocol: Some(SocketProtocol::Tcp),
            family: Some(if address.is_ipv4() {
                IpFamily::Ipv4
            } else {
                IpFamily::Ipv6
            }),
            local_address: Some(address.ip()),
            local_port: Some(address.port()),
            pid: Some(std::process::id()),
            ..Default::default()
        };
        let r = sockets(ConnectionFilter {
            state: Some("listen".into()),
            ..filter.clone()
        });
        assert_eq!(entries(&r).len(), 1);
        assert_eq!(entries(&r)[0].remote_address, None);
        let client = TcpStream::connect(address).unwrap();
        let (server, _) = listener.accept().unwrap();
        let r = sockets(ConnectionFilter {
            state: Some("established".into()),
            ..filter.clone()
        });
        assert_eq!(entries(&r).len(), 1);
        assert_eq!(
            entries(&r)[0].remote_port,
            Some(client.local_addr().unwrap().port())
        );
        drop(client);
        drop(server);
        drop(listener);
        let r = sockets(ConnectionFilter {
            state: Some("listen".into()),
            ..filter
        });
        assert!(entries(&r).is_empty());
    }
}
#[test]
fn native_udp_has_no_fake_peer_or_state_and_exact_filters_work() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    let filter = ConnectionFilter {
        protocol: Some(SocketProtocol::Udp),
        local_address: Some(address.ip()),
        local_port: Some(address.port()),
        pid: Some(std::process::id()),
        ..Default::default()
    };
    let r = sockets(filter.clone());
    assert_eq!(entries(&r).len(), 1);
    let item = &entries(&r)[0];
    assert_eq!(item.remote_address, None);
    assert_eq!(item.remote_port, None);
    assert_eq!(item.state, None);
    assert!(
        entries(&sockets(ConnectionFilter {
            pid: Some(u32::MAX),
            ..filter.clone()
        }))
        .is_empty()
    );
    drop(socket);
    assert!(entries(&sockets(filter)).is_empty());
}
struct Fixture {
    addr: std::net::SocketAddr,
    job: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.job.abort();
    }
}
impl Fixture {
    async fn new(code: ResponseCode, repeat: usize, silent: bool) -> Self {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let job = tokio::spawn(async move {
            let mut buf = [0u8; 8192];
            loop {
                let (len, peer) = socket.recv_from(&mut buf).await.unwrap();
                if silent {
                    continue;
                }
                let req = Message::from_vec(&buf[..len]).unwrap();
                let q = req.queries[0].clone();
                let mut response = req.into_response();
                response.metadata.response_code = code;
                response.metadata.authoritative = true;
                response.metadata.recursion_available = true;
                let target = Name::from_ascii("target.fixture.").unwrap();
                if code == ResponseCode::NoError {
                    for i in 0..repeat {
                        let data = match q.query_type {
                            RecordType::A => {
                                RData::A(A(std::net::Ipv4Addr::new(192, 0, 2, (i + 1) as u8)))
                            }
                            RecordType::AAAA => RData::AAAA(AAAA("2001:db8::1".parse().unwrap())),
                            RecordType::CNAME => RData::CNAME(CNAME(target.clone())),
                            RecordType::NS => RData::NS(NS(target.clone())),
                            RecordType::PTR => RData::PTR(PTR(target.clone())),
                            RecordType::MX => RData::MX(MX::new(10, target.clone())),
                            RecordType::SRV => RData::SRV(SRV::new(0, 1, 443, target.clone())),
                            RecordType::SOA => {
                                RData::SOA(SOA::new(target.clone(), target.clone(), 1, 2, 3, 4, 5))
                            }
                            RecordType::TXT => RData::TXT(TXT::new(vec!["test value".into()])),
                            _ => continue,
                        };
                        response.add_answer(Record::from_rdata(q.name.clone(), 42, data));
                    }
                }
                socket
                    .send_to(&response.to_vec().unwrap(), peer)
                    .await
                    .unwrap();
            }
        });
        Self { addr, job }
    }
    fn builder(&self) -> hickory_resolver::ResolverBuilder<TokioRuntimeProvider> {
        let mut ns = NameServerConfig::udp(self.addr.ip());
        ns.connections[0].port = self.addr.port();
        TokioResolver::builder_with_config(
            ResolverConfig::from_name_servers(vec![ns]),
            TokioRuntimeProvider::default(),
        )
    }
    async fn lookup(
        &self,
        name: &str,
        kind: DnsRecordType,
        limit: u16,
        timeout_ms: u32,
    ) -> SystemQueryReply {
        let q = SystemQuery::Dns {
            name: name.into(),
            record_type: kind,
            limit,
            timeout_ms,
        };
        let mut r = SystemQueryReply::pending(RequestId::new(), &q);
        r.sampled_from_unix_ms = Some(now());
        dns_with_builder(r, name, kind, timeout_ms, limit, self.builder()).await
    }
}
#[tokio::test]
async fn local_dns_types_ttl_and_ptr_source_are_preserved() {
    let f = Fixture::new(ResponseCode::NoError, 1, false).await;
    for kind in [
        DnsRecordType::A,
        DnsRecordType::Aaaa,
        DnsRecordType::Cname,
        DnsRecordType::Mx,
        DnsRecordType::Ns,
        DnsRecordType::Ptr,
        DnsRecordType::Soa,
        DnsRecordType::Srv,
        DnsRecordType::Txt,
    ] {
        let name = if kind == DnsRecordType::Ptr {
            "192.0.2.1"
        } else {
            "example.fixture."
        };
        let r = f.lookup(name, kind, 20, 1000).await;
        assert_eq!(r.state, "completed", "{r:?}");
        assert_eq!(r.returned_count, 1);
        let Some(SystemQueryData::Dns { result }) = r.data else {
            panic!()
        };
        assert!(!result.hosts_file_consulted);
        assert_eq!(result.record_type, kind);
        assert!(!result.records[0].value.is_empty());
        assert!(result.records[0].ttl_seconds > 0 && result.records[0].ttl_seconds <= 42);
        if kind == DnsRecordType::Ptr {
            assert_eq!(result.query_name, "1.2.0.192.in-addr.arpa.");
        }
    }
}
#[tokio::test]
async fn local_dns_limits_nxdomain_servfail_empty_and_timeout_are_explicit() {
    let f = Fixture::new(ResponseCode::NoError, 5, false).await;
    let r = f
        .lookup("example.fixture.", DnsRecordType::A, 2, 1000)
        .await;
    assert_eq!(r.state, "completed", "{r:?}");
    assert_eq!(r.returned_count, 2);
    assert!(r.truncated);
    assert_eq!(r.stop_reason.as_deref(), Some("entry_limit"));
    for (code, repeat, silent) in [
        (ResponseCode::NXDomain, 0, false),
        (ResponseCode::ServFail, 0, false),
        (ResponseCode::NoError, 0, false),
        (ResponseCode::NoError, 0, true),
    ] {
        let f = Fixture::new(code, repeat, silent).await;
        let start = Instant::now();
        let r = f
            .lookup("example.fixture.", DnsRecordType::A, 20, 200)
            .await;
        assert_eq!(r.state, "failed", "{r:?}");
        assert!(r.data.is_none());
        assert!(r.error.is_some());
        assert!(start.elapsed() < Duration::from_secs(2));
        if silent {
            assert!(r.error.unwrap().to_lowercase().contains("tim"));
        }
    }
}
#[tokio::test]
async fn sessions_filter_and_inaccessible_fields_do_not_invent_users() {
    let q = SystemQuery::Sessions {
        user: None,
        state: None,
        limit: 1,
    };
    let make = |id: &str, user: Option<&str>, state: &str| OsSessionInfo {
        id: id.into(),
        user_name: user.map(str::to_owned),
        user_id: None,
        domain: None,
        state: state.into(),
        active: None,
        remote: None,
        seat: None,
        terminal: None,
        client_name: None,
        session_type: None,
        errors: vec!["user permission denied".into()],
    };
    let batch = pab_os_sessions::SessionBatch {
        backend: "fixture",
        entries: vec![
            make("0", None, "listen"),
            make("1", Some("alice"), "active"),
            make("2", Some("bob"), "disconnected"),
        ],
        truncated: false,
    };
    let r = session_reply(
        SystemQueryReply::pending(RequestId::new(), &q),
        batch,
        Some("alice"),
        Some("active"),
        1,
    );
    assert_eq!(r.state, "completed");
    assert_eq!(r.returned_count, 1);
    assert!(!r.truncated);
    let Some(SystemQueryData::Sessions { entries, .. }) = r.data else {
        panic!()
    };
    assert_eq!(entries[0].id, "1");
    assert_eq!(entries[0].active, None);
    assert!(!entries[0].errors.is_empty());
    #[cfg(windows)]
    {
        let r = query_async(
            RequestId::new(),
            &SystemQuery::Sessions {
                user: Some("absent-pab-test-user-fixture".into()),
                state: None,
                limit: 100,
            },
        )
        .await
        .unwrap();
        assert_eq!(r.state, "completed", "{r:?}");
        assert_eq!(r.returned_count, 0);
    }
}
#[test]
fn c2_serialized_lists_use_same_output_budget_and_report_actual_count() {
    let q = SystemQuery::Sessions {
        user: None,
        state: None,
        limit: 1000,
    };
    let mut r = SystemQueryReply::pending(RequestId::new(), &q);
    r.state = "completed".into();
    r.data = Some(SystemQueryData::Sessions {
        backend: "fixture".into(),
        entries: (0..1000)
            .map(|i| OsSessionInfo {
                id: i.to_string(),
                user_name: Some("中文\"".repeat(100)),
                user_id: None,
                domain: None,
                state: "unknown".into(),
                active: None,
                remote: None,
                seat: None,
                terminal: None,
                client_name: None,
                session_type: None,
                errors: vec![],
            })
            .collect(),
    });
    bound_reply(&mut r);
    assert!(r.truncated);
    assert!(serde_json::to_vec(&r).unwrap().len() <= MAX_SYSTEM_REPLY_BYTES);
    assert!(r.returned_count < 1000);
}
