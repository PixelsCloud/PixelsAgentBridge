//! Isolated deployment acceptance driver, never a production MCP tool.
//! Provision exactly two fresh endpoint pairs in a disposable database first.
//! Private TOML supplies their keys; stdout contains only counts and receipts.
use iroh_base::SecretKey;
use pab_agent_core::{
    EndpointControlConfig, EndpointControlSupervisor, ReconnectPolicy, tls_connector,
};
use pab_protocol::{DeviceId, EndpointProofPrincipal, TenantId, UserId};
use pab_transport::{ConnectionPath, PabEndpoint, PabEndpointAddress, PabEndpointConfig};
use serde::Deserialize;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    control_url: String,
    ca_cert: PathBuf,
    tenant_id: TenantId,
    user_id: UserId,
    instructions_file: PathBuf,
    pairs: Vec<Pair>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    relay_url: String,
    operator_seed: String,
    device_seed: String,
    device_id: DeviceId,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Instruction {
    id: uuid::Uuid,
    target_url: String,
}
fn key(value: &str) -> Result<SecretKey> {
    let bytes: [u8; 32] = hex::decode(value)?.try_into().map_err(|_| "seed length")?;
    Ok(SecretKey::from_bytes(&bytes))
}
async fn home(endpoint: &PabEndpoint, url: &str) -> Result<()> {
    let mut watch = endpoint.watch_address();
    tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            if watch.borrow().relay_urls == [url.to_owned()] {
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(());
            }
            watch.changed().await?;
        }
    })
    .await??;
    Ok(())
}
async fn exchange(a: &PabEndpoint, b: &PabEndpoint, large: bool) -> Result<()> {
    let address = PabEndpointAddress {
        relay_urls: b.address().relay_urls().map(ToString::to_string).collect(),
        direct_addresses: vec![],
    };
    tokio::time::timeout(Duration::from_secs(180), async {
        let (outbound, inbound) = tokio::join!(
            a.connect(*b.id().as_bytes(), &address, Duration::from_secs(25)),
            b.accept()
        );
        let outbound = outbound?;
        let inbound = inbound?.ok_or("endpoint closed")?;
        if outbound.selected_path() != Some(ConnectionPath::Relay) {
            return Err("test unexpectedly used P2P".into());
        }
        let count = if large { 512 } else { 1 };
        let size = if large { 65536 } else { 128 };
        let send = async {
            let mut stream = outbound.open_bi(Duration::from_secs(10)).await?;
            let mut digest = blake3::Hasher::new();
            for index in 0..count {
                let bytes: Vec<u8> = (0..size)
                    .map(|offset| (offset as u8).wrapping_mul(71) ^ (index as u8))
                    .collect();
                digest.update(&bytes);
                stream
                    .send_binary_frame(&bytes, Duration::from_secs(25))
                    .await?;
            }
            let receipt: Vec<u8> = stream.receive_json(Duration::from_secs(25)).await?;
            if receipt != digest.finalize().as_bytes() {
                return Err("application receipt hash mismatch".into());
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        };
        let receive = async {
            let mut stream = inbound.accept_bi(Duration::from_secs(10)).await?;
            let mut digest = blake3::Hasher::new();
            for _ in 0..count {
                let bytes = stream.receive_binary_frame(Duration::from_secs(25)).await?;
                if bytes.len() != size {
                    return Err("frame length mismatch".into());
                }
                digest.update(&bytes);
            }
            stream
                .send_json(
                    &digest.finalize().as_bytes().to_vec(),
                    Duration::from_secs(10),
                )
                .await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        };
        let result = tokio::try_join!(send, receive);
        outbound.close(b"acceptance receipt");
        inbound.close(b"acceptance receipt");
        result?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    })
    .await??;
    Ok(())
}
#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let path = std::env::args_os()
        .nth(1)
        .ok_or("private TOML path required")?;
    if std::fs::metadata(&path)?.len() > 65536 {
        return Err("configuration too large".into());
    }
    let config: Config = pab_service_config::load(std::path::Path::new(&path))?;
    pab_logging::init(
        "relay-acceptance",
        config
            .instructions_file
            .parent()
            .ok_or("missing fixture directory")?,
    )?;
    if config.pairs.len() != 2 {
        return Err("exactly two isolated pairs required".into());
    }
    let ca = std::fs::read(&config.ca_cert)?;
    let connector = tls_connector(Some(&ca))?;
    let mut endpoints = Vec::new();
    let mut controls = Vec::new();
    for pair in &config.pairs {
        let ep_config = PabEndpointConfig::new(vec![pair.relay_url.parse()?])?
            .with_extra_ca_pem(&ca)?
            .relay_only_for_testing();
        for (seed, principal) in [
            (
                &pair.operator_seed,
                EndpointProofPrincipal::User {
                    user_id: config.user_id,
                },
            ),
            (
                &pair.device_seed,
                EndpointProofPrincipal::Device {
                    device_id: pair.device_id,
                },
            ),
        ] {
            let secret = key(seed)?;
            let endpoint = PabEndpoint::bind(ep_config.clone(), secret.clone()).await?;
            let control = EndpointControlSupervisor::new(
                EndpointControlConfig {
                    url: config.control_url.clone(),
                    tenant_id: config.tenant_id,
                    principal,
                    operation_timeout: Duration::from_secs(10),
                },
                secret,
                connector.clone(),
                ReconnectPolicy {
                    heartbeat_interval: Duration::from_secs(5),
                    heartbeat_timeout: Duration::from_secs(2),
                    retry_interval: Duration::from_millis(300),
                },
            )?
            .with_relay_directory()
            .spawn();
            endpoints.push(endpoint);
            controls.push(control);
        }
    }
    for (index, endpoint) in endpoints.iter().enumerate() {
        home(endpoint, &config.pairs[index / 2].relay_url).await?;
    }
    eprintln!("Both isolated endpoint pairs are connected to their configured home Relays");
    let identities: Vec<_> = endpoints.iter().map(PabEndpoint::id).collect();
    let started = Instant::now();
    tokio::try_join!(
        exchange(&endpoints[0], &endpoints[1], true),
        exchange(&endpoints[2], &endpoints[3], true)
    )?;
    if started.elapsed() < Duration::from_secs(50) {
        return Err("64 MiB exceeded the 10 Mbps aggregate account bound".into());
    }
    println!(
        "{}",
        serde_json::json!({"phase":"transfer_complete","bytes":67108864,"pairs":2,"elapsed_ms":started.elapsed().as_millis(),"hashes_verified":true,"relay_only":true})
    );
    for (endpoint, control) in endpoints.iter().zip(&controls) {
        endpoint.follow_relay_updates(control.relay_updates());
    }
    let mut previous = uuid::Uuid::nil();
    let mut idle_since = Instant::now();
    loop {
        if idle_since.elapsed() > Duration::from_secs(120) {
            return Err("coordinator stopped sending instructions".into());
        }
        if config.instructions_file.exists() {
            if std::fs::metadata(&config.instructions_file)?.len() > 4096 {
                return Err("instruction too large".into());
            }
            let instruction: Instruction =
                serde_json::from_slice(&std::fs::read(&config.instructions_file)?)?;
            if instruction.id != previous {
                previous = instruction.id;
                if instruction.target_url == "exit" {
                    break;
                }
                if !config
                    .pairs
                    .iter()
                    .any(|p| p.relay_url == instruction.target_url)
                {
                    return Err("unknown target URL".into());
                }
                let started = Instant::now();
                for endpoint in &endpoints {
                    home(endpoint, &instruction.target_url).await?;
                }
                tokio::try_join!(
                    exchange(&endpoints[0], &endpoints[1], false),
                    exchange(&endpoints[2], &endpoints[3], false)
                )?;
                if endpoints.iter().map(PabEndpoint::id).collect::<Vec<_>>() != identities {
                    return Err("endpoint identities changed".into());
                }
                if started.elapsed() > Duration::from_secs(30) {
                    return Err("failover exceeded 30 seconds".into());
                }
                println!(
                    "{}",
                    serde_json::json!({"phase":"switched","id":instruction.id,"elapsed_ms":started.elapsed().as_millis(),"hashes_verified":true,"identities_unchanged":true})
                );
                idle_since = Instant::now();
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    for endpoint in endpoints {
        endpoint.close().await;
    }
    for control in controls {
        control.shutdown().await?;
    }
    Ok(())
}
