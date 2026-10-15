//! Readiness probe with ephemeral, unregistered identities and a bounded permit.
//! Reads credentials from private TOML; never receives passwords as CLI arguments.
use iroh_base::{RelayUrl, SecretKey};
use iroh_dns::dns::DnsResolver;
use iroh_relay::{
    client::{Client, ClientBuilder},
    protos::relay::{ClientToRelayMsg, Datagrams, RelayToClientMsg},
    tls::{CaTlsConfig, default_provider},
};
use n0_future::{SinkExt, StreamExt};
use pab_protocol::EndpointKey;
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
    node_id: String,
    ca_cert: Option<PathBuf>,
    relay_ca_cert: Option<PathBuf>,
    auth: Auth,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Auth {
    username: Option<String>,
    password: Option<String>,
    access_token_file: Option<PathBuf>,
}
#[derive(Deserialize)]
struct Receipt {
    probe_id: uuid::Uuid,
    url: String,
    applied: bool,
    expired: bool,
}

async fn response<T: serde::de::DeserializeOwned>(request: reqwest::RequestBuilder) -> Result<T> {
    let mut reply = request.send().await?;
    if !reply.status().is_success() {
        return Err(format!(
            "management probe request failed: HTTP {}",
            reply.status().as_u16()
        )
        .into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = reply.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > 65536 {
            return Err("management probe response exceeds 64 KiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}
fn roots(path: &std::path::Path) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    if std::fs::metadata(path)?.len() > 1024 * 1024 {
        return Err("probe CA file exceeds 1 MiB".into());
    }
    let bytes = std::fs::read(path)?;
    let result =
        rustls_pemfile::certs(&mut bytes.as_slice()).collect::<std::io::Result<Vec<_>>>()?;
    if result.is_empty() {
        return Err("probe CA file contains no certificates".into());
    }
    Ok(result)
}
async fn receive(
    client: &mut Client,
    expected: iroh_base::EndpointId,
    payload: &[u8],
) -> Result<()> {
    loop {
        match client.next().await.ok_or("probe connection closed")?? {
            RelayToClientMsg::Datagrams {
                remote_endpoint_id,
                datagrams,
            } => {
                if remote_endpoint_id != expected || datagrams.contents.as_ref() != payload {
                    return Err("probe payload or source mismatch".into());
                }
                return Ok(());
            }
            RelayToClientMsg::Ping(value) => client.send(ClientToRelayMsg::Pong(value)).await?,
            RelayToClientMsg::Pong(_) | RelayToClientMsg::Health { .. } => {}
            _ => return Err("Relay changed state during probe".into()),
        }
    }
}
async fn exchange(
    api: &reqwest::Client,
    origin: &str,
    token: &str,
    receipt: &mut Receipt,
    config: &Config,
    first: SecretKey,
    second: SecretKey,
) -> Result<serde_json::Value> {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !receipt.applied {
        if receipt.expired || Instant::now() >= deadline {
            return Err("Relay did not accept its probe permit in time".into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        *receipt = response(
            api.get(format!(
                "{origin}/api/admin/relay-probes/{}",
                receipt.probe_id
            ))
            .bearer_auth(token),
        )
        .await?;
    }
    let url: RelayUrl = receipt.url.parse()?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err("invalid probe Relay URL".into());
    }
    let mut tls = CaTlsConfig::embedded();
    if let Some(path) = config.relay_ca_cert.as_ref().or(config.ca_cert.as_ref()) {
        tls = tls.with_extra_roots(roots(path)?);
    }
    let tls = tls.client_config(default_provider())?;
    let first_id = first.public();
    let second_id = second.public();
    let mut a = ClientBuilder::new(url.clone(), first, DnsResolver::new())
        .tls_client_config(tls.clone())
        .connect()
        .await?;
    let mut b = ClientBuilder::new(url, second, DnsResolver::new())
        .tls_client_config(tls)
        .connect()
        .await?;
    let nonce = uuid::Uuid::new_v4();
    let bytes: Vec<_> = (0..4096)
        .map(|i| nonce.as_bytes()[i % 16] ^ (i as u8))
        .collect();
    let started = Instant::now();
    a.send(ClientToRelayMsg::Datagrams {
        dst_endpoint_id: second_id,
        datagrams: Datagrams::from(&bytes),
    })
    .await?;
    receive(&mut b, first_id, &bytes).await?;
    b.send(ClientToRelayMsg::Datagrams {
        dst_endpoint_id: first_id,
        datagrams: Datagrams::from(&bytes),
    })
    .await?;
    receive(&mut a, second_id, &bytes).await?;
    Ok(
        serde_json::json!({"ok":true,"node_id":config.node_id,"bytes":8192,"round_trip_ms":started.elapsed().as_millis(),"digest":blake3::hash(&bytes).to_hex().to_string()}),
    )
}
#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (path, rest) =
        pab_service_config::arguments("relay-healthcheck.toml", std::env::args_os().skip(1))?;
    if !rest.is_empty() {
        return Err("usage: pab-relay-healthcheck --config private.toml".into());
    }
    if std::fs::metadata(&path)?.len() > 1024 * 1024 {
        return Err("probe configuration exceeds 1 MiB".into());
    }
    let mut config: Config = pab_service_config::load(&path)?;
    let base = path.parent().unwrap_or(std::path::Path::new("."));
    for path in [
        &mut config.ca_cert,
        &mut config.relay_ca_cert,
        &mut config.auth.access_token_file,
    ]
    .into_iter()
    .flatten()
    {
        pab_service_config::resolve(base, path);
    }
    let url = reqwest::Url::parse(&config.control_url)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.path() != "/"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !pab_protocol::valid_relay_node_id(&config.node_id)
    {
        return Err("probe requires an HTTPS control origin and valid node ID".into());
    }
    let origin = url.origin().ascii_serialization();
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    if let Some(path) = &config.ca_cert {
        for cert in roots(path)? {
            builder = builder.add_root_certificate(reqwest::Certificate::from_der(cert.as_ref())?);
        }
    }
    let api = builder.build()?;
    let owned = config.auth.access_token_file.is_none();
    let token = if let Some(path) = &config.auth.access_token_file {
        if std::fs::metadata(path)?.len() > 16384 {
            return Err("probe token file is too large".into());
        }
        std::fs::read_to_string(path)?.trim().to_string()
    } else {
        let user = config
            .auth
            .username
            .as_ref()
            .ok_or("probe username is missing")?;
        let password = config
            .auth
            .password
            .as_ref()
            .ok_or("probe password is missing")?;
        let reply: serde_json::Value = response(
            api.post(format!("{origin}/api/account/session"))
                .json(&serde_json::json!({"username":user,"password":password})),
        )
        .await?;
        reply["access_token"]
            .as_str()
            .ok_or("probe login did not return a token")?
            .to_owned()
    };
    let result:Result<serde_json::Value>=async{
        let first=SecretKey::generate();let second=SecretKey::generate();
        let mut receipt:Receipt=response(api.post(format!("{origin}/api/admin/relays/{}/probe",config.node_id)).bearer_auth(&token)
            .json(&serde_json::json!({"first":EndpointKey::new(*first.public().as_bytes()),"second":EndpointKey::new(*second.public().as_bytes())}))).await?;
        let result=tokio::time::timeout(Duration::from_secs(15),exchange(&api,&origin,&token,&mut receipt,&config,first,second)).await;
        let _=api.delete(format!("{origin}/api/admin/relay-probes/{}",receipt.probe_id)).bearer_auth(&token).send().await;
        result.map_err(|_|"Relay health probe timed out")?
    }.await;
    if owned {
        let _ = api
            .post(format!("{origin}/api/account/logout"))
            .bearer_auth(&token)
            .send()
            .await;
    }
    println!("{}", result?);
    Ok(())
}
