use std::{
    collections::{HashMap, HashSet},
    env,
    fs::File,
    io::{self, BufReader, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use iroh_base::{EndpointId, RelayUrl, SecretKey};
use iroh_dns::dns::DnsResolver;
use iroh_relay::{
    client::{Client, ClientBuilder},
    protos::relay::{ClientToRelayMsg, Datagrams, RelayToClientMsg},
    quic::QuicClient,
    server::{
        Access, AccessControl, CertConfig, ClientRequest, QuicConfig, RelayConfig, Server,
        ServerConfig, TlsConfig,
    },
    tls::{CaTlsConfig, default_provider},
};
use n0_future::{SinkExt, StreamExt};
use pab_protocol::{TrafficScope, mbps_to_bytes_per_second};
use pab_relay::{Acquire, AggregateLimiter, LimitKey, Rate};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
struct EndpointAllowlist(HashSet<EndpointId>);

impl AccessControl for EndpointAllowlist {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        if self.0.contains(&request.endpoint_id()) {
            Access::Allow
        } else {
            Access::Deny {
                reason: Some("endpoint is not registered for this probe".to_owned()),
            }
        }
    }
}

#[derive(Debug)]
struct TeamForwarding {
    limiter: Mutex<AggregateLimiter>,
    members: HashMap<EndpointId, u64>,
}

impl TeamForwarding {
    fn new(members: HashMap<EndpointId, u64>, team_mbps: u32, member_mbps: u32) -> Result<Self> {
        let now = Instant::now();
        let burst = Duration::from_millis(150);
        let mut limiter = AggregateLimiter::default();
        let team_rate =
            Rate::new(mbps_to_bytes_per_second(team_mbps), burst).ok_or("invalid Team rate")?;
        let member_rate =
            Rate::new(mbps_to_bytes_per_second(member_mbps), burst).ok_or("invalid member rate")?;
        limiter.set_rate(LimitKey::Team(1), team_rate, now);
        for user_id in members.values().copied().collect::<HashSet<_>>() {
            limiter.set_rate(
                LimitKey::Member {
                    team_id: 1,
                    user_id,
                },
                member_rate,
                now,
            );
        }
        Ok(Self {
            limiter: Mutex::new(limiter),
            members,
        })
    }
}

impl iroh_relay::server::ForwardingControl for TeamForwarding {
    fn check(
        &self,
        src: EndpointId,
        dst: EndpointId,
        bytes: usize,
    ) -> iroh_relay::server::ForwardingDecision {
        use iroh_relay::server::ForwardingDecision;
        let user_id = match (self.members.get(&src), self.members.get(&dst)) {
            (Some(src_user), None) => *src_user,
            (None, Some(dst_user)) => *dst_user,
            (Some(src_user), Some(dst_user)) if src_user == dst_user => *src_user,
            _ => return ForwardingDecision::Drop,
        };
        let bytes = match u64::try_from(bytes) {
            Ok(bytes) => bytes,
            Err(_) => return ForwardingDecision::Drop,
        };
        match self.limiter.lock().expect("limiter lock").acquire(
            TrafficScope::Team {
                team_id: 1,
                user_id,
            },
            bytes,
            Instant::now(),
        ) {
            Acquire::Ready => ForwardingDecision::Allow,
            Acquire::Wait(delay) => ForwardingDecision::Wait(delay),
            Acquire::Oversized { .. } | Acquire::Missing(_) => ForwardingDecision::Drop,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("endpoint-id") => {
            let secret = parse_secret(required(&args, 2, "secret hex")?)?;
            println!("{}", secret.public());
        }
        Some("server") => run_server(&args).await?,
        Some("server-rate") => run_rate_server(&args).await?,
        Some("ping") => run_ping(&args).await?,
        Some("qad") => run_qad(&args).await?,
        Some("receive") => run_receive(&args).await?,
        Some("send") => run_send(&args).await?,
        Some("load") => run_load(&args).await?,
        Some("sink") => run_sink(&args).await?,
        _ => print_usage(),
    }
    Ok(())
}

async fn run_server(args: &[String]) -> Result<()> {
    let cert_path = required(args, 2, "certificate path")?;
    let key_path = required(args, 3, "private key path")?;
    let allowed = required(args, 4, "comma-separated endpoint IDs")?
        .split(',')
        .map(EndpointId::from_str)
        .collect::<std::result::Result<HashSet<_>, _>>()?;
    if allowed.is_empty() {
        return Err("at least one endpoint must be allowed".into());
    }
    run_server_with(
        cert_path,
        key_path,
        allowed,
        Arc::new(iroh_relay::server::AllowAllForwarding),
        parse_addr(args.get(5), Ipv4Addr::LOCALHOST, 31_443)?,
        parse_addr(args.get(6), Ipv4Addr::LOCALHOST, 31_444)?,
        parse_addr(args.get(7), Ipv4Addr::UNSPECIFIED, 7_842)?,
    )
    .await
}

async fn run_rate_server(args: &[String]) -> Result<()> {
    let cert_path = required(args, 2, "certificate path")?;
    let key_path = required(args, 3, "private key path")?;
    let executors = required(args, 4, "comma-separated executor endpoint IDs")?
        .split(',')
        .map(EndpointId::from_str)
        .collect::<std::result::Result<HashSet<_>, _>>()?;
    if executors.is_empty() {
        return Err("at least one executor endpoint is required".into());
    }
    let members = required(args, 5, "endpoint:user mappings")?
        .split(',')
        .map(|mapping| {
            let (endpoint, user) = mapping
                .split_once(':')
                .ok_or("member mapping must be endpoint:user")?;
            Ok((EndpointId::from_str(endpoint)?, user.parse::<u64>()?))
        })
        .collect::<Result<HashMap<_, _>>>()?;
    if members.is_empty() {
        return Err("at least one member endpoint is required".into());
    }
    let team_mbps = required(args, 6, "Team Mbps")?.parse::<u32>()?;
    let member_mbps = required(args, 7, "member Mbps")?.parse::<u32>()?;
    let allowed = members
        .keys()
        .copied()
        .chain(executors)
        .collect::<HashSet<_>>();
    let forwarding = Arc::new(TeamForwarding::new(members, team_mbps, member_mbps)?);
    run_server_with(
        cert_path,
        key_path,
        allowed,
        forwarding,
        parse_addr(args.get(8), Ipv4Addr::LOCALHOST, 31_443)?,
        parse_addr(args.get(9), Ipv4Addr::LOCALHOST, 31_444)?,
        parse_addr(args.get(10), Ipv4Addr::UNSPECIFIED, 7_842)?,
    )
    .await
}

async fn run_server_with(
    cert_path: &str,
    key_path: &str,
    allowed: HashSet<EndpointId>,
    forwarding: Arc<dyn iroh_relay::server::DynForwardingControl>,
    https_addr: SocketAddr,
    captive_addr: SocketAddr,
    quic_addr: SocketAddr,
) -> Result<()> {
    let tls = load_server_tls(cert_path, key_path)?;

    let mut relay = RelayConfig::new(captive_addr);
    relay.tls = Some(TlsConfig::new(
        https_addr,
        CertConfig::Manual {
            server_config: tls.clone(),
        },
    ));
    relay.access = Arc::new(EndpointAllowlist(allowed));
    relay.forwarding = forwarding;
    let mut quic = QuicConfig::new(quic_addr);
    quic.server_config = Some(tls);
    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    config.quic = Some(quic);

    let server = Server::spawn(config).await?;
    println!(
        "READY https={} captive={} quic={}",
        server.https_addr().ok_or("missing HTTPS listener")?,
        server.http_addr().ok_or("missing captive listener")?,
        server.quic_addr().ok_or("missing QUIC listener")?
    );
    io::stdout().flush()?;
    tokio::signal::ctrl_c().await?;
    server.shutdown().await?;
    Ok(())
}

async fn run_ping(args: &[String]) -> Result<()> {
    let (mut client, endpoint_id) = connect(
        required(args, 2, "relay URL")?,
        required(args, 3, "secret hex")?,
    )
    .await?;
    let payload = *b"PABPING1";
    client.send(ClientToRelayMsg::Ping(payload)).await?;
    let message = next_message(&mut client).await?;
    match message {
        RelayToClientMsg::Pong(value) if value == payload => {
            println!("PONG endpoint={endpoint_id}");
            Ok(())
        }
        other => Err(format!("unexpected ping response: {other:?}").into()),
    }
}

async fn run_qad(args: &[String]) -> Result<()> {
    let server_addr: SocketAddr = required(args, 2, "QUIC server address")?.parse()?;
    let tls_name = required(args, 3, "TLS server name")?;
    let endpoint = noq::Endpoint::client((Ipv4Addr::UNSPECIFIED, 0).into())?;
    let tls = CaTlsConfig::embedded().client_config(default_provider())?;
    let client = QuicClient::new(endpoint.clone(), tls);
    let connection = tokio::time::timeout(
        Duration::from_secs(15),
        client.create_conn(server_addr, tls_name),
    )
    .await
    .map_err(|_| "QUIC address discovery connection timed out")??;
    let mut observed = connection.observed_external_addr();
    let observed_addr = tokio::time::timeout(Duration::from_secs(15), observed.next())
        .await
        .map_err(|_| "QUIC observed-address response timed out")?
        .ok_or("QUIC observed-address stream ended")?;
    println!("QAD observed={observed_addr} server={server_addr}");
    connection.close(0u32.into(), b"probe complete");
    endpoint.wait_idle().await;
    Ok(())
}

async fn run_receive(args: &[String]) -> Result<()> {
    let (mut client, endpoint_id) = connect(
        required(args, 2, "relay URL")?,
        required(args, 3, "secret hex")?,
    )
    .await?;
    println!("READY endpoint={endpoint_id}");
    io::stdout().flush()?;
    loop {
        match next_message(&mut client).await? {
            RelayToClientMsg::Datagrams {
                remote_endpoint_id,
                datagrams,
            } => {
                let bytes = datagrams.contents;
                println!(
                    "RECEIVED from={} bytes={} text={}",
                    remote_endpoint_id,
                    bytes.len(),
                    String::from_utf8_lossy(&bytes)
                );
                return Ok(());
            }
            RelayToClientMsg::Ping(payload) => {
                client.send(ClientToRelayMsg::Pong(payload)).await?;
            }
            _ => {}
        }
    }
}

async fn run_send(args: &[String]) -> Result<()> {
    let (mut client, endpoint_id) = connect(
        required(args, 2, "relay URL")?,
        required(args, 3, "secret hex")?,
    )
    .await?;
    let destination = EndpointId::from_str(required(args, 4, "destination endpoint ID")?)?;
    let message = required(args, 5, "message")?;
    client
        .send(ClientToRelayMsg::Datagrams {
            dst_endpoint_id: destination,
            datagrams: Datagrams::from(message.as_bytes()),
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    println!(
        "SENT from={} to={} bytes={}",
        endpoint_id,
        destination,
        message.len()
    );
    Ok(())
}

async fn run_load(args: &[String]) -> Result<()> {
    let (mut client, endpoint_id) = connect(
        required(args, 2, "relay URL")?,
        required(args, 3, "secret hex")?,
    )
    .await?;
    let destination = EndpointId::from_str(required(args, 4, "destination endpoint ID")?)?;
    let seconds = required(args, 5, "duration seconds")?.parse::<u64>()?;
    let payload_size = args
        .get(6)
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(1_200);
    if !(1..=60_000).contains(&payload_size) {
        return Err("payload size must be between 1 and 60000 bytes".into());
    }
    let payload = vec![0x5a; payload_size];
    let started = Instant::now();
    let deadline = started + Duration::from_secs(seconds);
    let mut bytes = 0u64;
    while Instant::now() < deadline {
        let message = ClientToRelayMsg::Datagrams {
            dst_endpoint_id: destination,
            datagrams: Datagrams::from(&payload),
        };
        match tokio::time::timeout(Duration::from_secs(2), client.send(message)).await {
            Ok(result) => result?,
            Err(_) => break,
        }
        bytes = bytes.saturating_add(payload_size as u64);
    }
    let elapsed = started.elapsed();
    println!(
        "LOAD endpoint={} bytes={} elapsed_ms={} submitted_mbps={:.3}",
        endpoint_id,
        bytes,
        elapsed.as_millis(),
        bits_per_second(bytes, elapsed) / 1_000_000.0
    );
    Ok(())
}

async fn run_sink(args: &[String]) -> Result<()> {
    let (mut client, endpoint_id) = connect(
        required(args, 2, "relay URL")?,
        required(args, 3, "secret hex")?,
    )
    .await?;
    let seconds = required(args, 4, "duration seconds")?.parse::<u64>()?;
    println!("READY endpoint={endpoint_id}");
    io::stdout().flush()?;
    let started = Instant::now();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    let mut by_source: HashMap<EndpointId, u64> = HashMap::new();
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            message = client.next() => {
                match message.ok_or("relay stream ended")?? {
                    RelayToClientMsg::Datagrams { remote_endpoint_id, datagrams } => {
                        let bytes = u64::try_from(datagrams.contents.len()).unwrap_or(u64::MAX);
                        by_source.entry(remote_endpoint_id)
                            .and_modify(|total| *total = total.saturating_add(bytes))
                            .or_insert(bytes);
                    }
                    RelayToClientMsg::Ping(payload) => {
                        client.send(ClientToRelayMsg::Pong(payload)).await?;
                    }
                    _ => {}
                }
            }
        }
    }
    let elapsed = started.elapsed();
    let total = by_source.values().copied().sum::<u64>();
    println!(
        "SINK endpoint={} bytes={} elapsed_ms={} mbps={:.3}",
        endpoint_id,
        total,
        elapsed.as_millis(),
        bits_per_second(total, elapsed) / 1_000_000.0
    );
    let mut sources = by_source.into_iter().collect::<Vec<_>>();
    sources.sort_unstable_by_key(|(source, _)| source.to_string());
    for (source, bytes) in sources {
        println!(
            "SOURCE endpoint={} bytes={} mbps={:.3}",
            source,
            bytes,
            bits_per_second(bytes, elapsed) / 1_000_000.0
        );
    }
    Ok(())
}

fn bits_per_second(bytes: u64, elapsed: Duration) -> f64 {
    if elapsed.is_zero() {
        return 0.0;
    }
    bytes as f64 * 8.0 / elapsed.as_secs_f64()
}

async fn connect(url: &str, secret_hex: &str) -> Result<(Client, EndpointId)> {
    let url: RelayUrl = url.parse()?;
    let secret = parse_secret(secret_hex)?;
    let endpoint_id = secret.public();
    let tls = CaTlsConfig::embedded().client_config(default_provider())?;
    let client = ClientBuilder::new(url, secret, DnsResolver::new())
        .tls_client_config(tls)
        .connect()
        .await?;
    Ok((client, endpoint_id))
}

async fn next_message(client: &mut Client) -> Result<RelayToClientMsg> {
    let next = tokio::time::timeout(Duration::from_secs(15), client.next())
        .await
        .map_err(|_| "relay response timed out")?;
    let message = match next {
        Some(message) => message,
        None => return Err("relay stream ended".into()),
    };
    Ok(message?)
}

fn load_server_tls(cert_path: &str, key_path: &str) -> Result<rustls::ServerConfig> {
    let mut cert_reader = BufReader::new(File::open(Path::new(cert_path))?);
    let certificates =
        rustls_pemfile::certs(&mut cert_reader).collect::<std::result::Result<Vec<_>, _>>()?;
    let mut key_reader = BufReader::new(File::open(Path::new(key_path))?);
    let key = rustls_pemfile::private_key(&mut key_reader)?.ok_or("private key not found")?;
    Ok(rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(certificates, key)?)
}

fn parse_secret(value: &str) -> Result<SecretKey> {
    let decoded = hex::decode(value)?;
    let bytes: [u8; 32] = decoded
        .try_into()
        .map_err(|_| "secret key must contain exactly 32 bytes")?;
    Ok(SecretKey::from_bytes(&bytes))
}

fn parse_addr(
    value: Option<&String>,
    default_ip: Ipv4Addr,
    default_port: u16,
) -> Result<SocketAddr> {
    match value {
        Some(value) => Ok(value.parse()?),
        None => Ok(SocketAddr::new(IpAddr::V4(default_ip), default_port)),
    }
}

fn required<'a>(args: &'a [String], index: usize, name: &str) -> Result<&'a str> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| format!("missing {name}").into())
}

fn print_usage() {
    eprintln!("pab-relay-probe endpoint-id <secret-hex>");
    eprintln!(
        "pab-relay-probe server <cert.pem> <key.pem> <endpoint-id,...> [https-addr] [captive-addr] [quic-addr]"
    );
    eprintln!(
        "pab-relay-probe server-rate <cert.pem> <key.pem> <executor-id,...> <endpoint:user,...> <team-mbps> <member-mbps> [https-addr] [captive-addr] [quic-addr]"
    );
    eprintln!("pab-relay-probe ping <relay-url> <secret-hex>");
    eprintln!("pab-relay-probe qad <server-ip:port> <tls-server-name>");
    eprintln!("pab-relay-probe receive <relay-url> <secret-hex>");
    eprintln!("pab-relay-probe send <relay-url> <secret-hex> <destination-id> <message>");
    eprintln!(
        "pab-relay-probe load <relay-url> <secret-hex> <destination-id> <seconds> [payload-bytes]"
    );
    eprintln!("pab-relay-probe sink <relay-url> <secret-hex> <seconds>");
}
