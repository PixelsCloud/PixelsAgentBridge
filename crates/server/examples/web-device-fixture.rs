//! Isolated Web E2E device. Never connects to a non-loopback deployment.
use pab_agent_core::{
    AuthenticatedControlConnection, EndpointControlConfig, OpenRegistrationKind,
    register_open_endpoint, tls_connector,
};
use pab_protocol::{
    DEVICE_SESSION_SCHEMA_VERSION, DeploymentId, DeviceHello, DeviceRef, EndpointProofPrincipal,
    EndpointRegistrationResult,
};
use serde_json::{Value, json};
use std::{io::Write, time::Duration};
use tokio::io::{AsyncBufReadExt, BufReader};

fn emit(value: Value) {
    println!("{value}");
    let _ = std::io::stdout().flush();
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [url, ca_path, deployment, name] = arguments.as_slice() else {
        return Err("url ca deployment name required".into());
    };
    let parsed = url::Url::parse(url)?;
    if parsed.scheme() != "wss" || !matches!(parsed.host_str(), Some("localhost" | "127.0.0.1")) {
        return Err("fixture requires loopback WSS".into());
    }
    let deployment_id = deployment.parse::<DeploymentId>()?;
    let ca = std::fs::read(ca_path)?;
    let connector = tls_connector(Some(&ca))?;
    let secret = iroh_base::SecretKey::generate();
    let registered = register_open_endpoint(
        url,
        deployment_id,
        &secret,
        OpenRegistrationKind::Device { name: name.clone() },
        connector.clone(),
        Duration::from_secs(10),
    )
    .await?;
    let EndpointRegistrationResult::Device {
        tenant_id,
        device_id,
        device_code,
        ..
    } = registered
    else {
        return Err("not a device".into());
    };
    let config = EndpointControlConfig {
        url: url.clone(),
        deployment_id,
        tenant_id,
        principal: EndpointProofPrincipal::Device { device_id },
        operation_timeout: Duration::from_secs(10),
    };
    let mut primary =
        Some(AuthenticatedControlConnection::connect(&config, &secret, connector.clone()).await?);
    let mut secondary: Option<AuthenticatedControlConnection> = None;
    primary
        .as_mut()
        .unwrap()
        .publish_device_hello(
            &DeviceHello {
                schema_version: DEVICE_SESSION_SCHEMA_VERSION,
                device_ref: DeviceRef {
                    deployment_id,
                    tenant_id,
                    device_id,
                },
                agent_version: "web-e2e".into(),
                observed_at_unix_ms: (time::OffsetDateTime::now_utc().unix_timestamp_nanos()
                    / 1_000_000) as i64,
                execution_context: pab_platform::detect_native_execution_context()?,
            },
            Duration::from_secs(10),
        )
        .await?;
    emit(json!({"ready":true,"device_id":device_id,"device_code":device_code}));
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut interval = tokio::time::interval(Duration::from_secs(10));
    loop {
        tokio::select! {
            _=interval.tick()=>{
                if let Some(connection)=primary.as_mut(){let _=connection.list_device_claims(Duration::from_secs(5)).await?;}
                if let Some(connection)=secondary.as_mut(){let _:Vec<pab_protocol::DeviceClaimEntry>=connection.list_device_claims(Duration::from_secs(5)).await?;}
            }
            line=lines.next_line()=>{
                let Some(line)=line? else{break;};let input:Value=serde_json::from_str(&line)?;
                match input["action"].as_str(){
                    Some("second")=>{secondary=Some(AuthenticatedControlConnection::connect(&config,&secret,connector.clone()).await?);},
                    Some("drop_second")=>{secondary=None;},
                    Some("disconnect")=>{primary=None;secondary=None;},
                    Some("reconnect")=>{primary=Some(AuthenticatedControlConnection::connect(&config,&secret,connector.clone()).await?);},
                    Some("approve")=>{
                        let claim=input["claim_id"].as_str().ok_or("claim id required")?.parse()?;
                        primary.as_mut().ok_or("offline")?.approve_device_claim(claim,Duration::from_secs(5)).await?;
                    },
                    Some("quit")=>break,
                    _=>return Err("unknown fixture action".into()),
                }
                emit(json!({"done":input["action"]}));
            }
        }
    }
    Ok(())
}
