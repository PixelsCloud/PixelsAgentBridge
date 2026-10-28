//! Isolate public control metadata compatibility without reading a phone's identity keys.
use iroh_base::SecretKey;
use pab_agent_core::{
    AuthenticatedControlConnection, ControlConnectionPhase, EndpointControlConfig,
    EndpointControlError, EndpointControlSupervisor, OpenRegistrationKind, ReconnectPolicy,
    register_open_endpoint, tls_connector,
};
use pab_protocol::*;
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn category(error: &EndpointControlError) -> String {
    match error {
        EndpointControlError::Server { code, .. } => format!("server:{code:?}"),
        EndpointControlError::Closed => "closed".into(),
        EndpointControlError::Timeout => "timeout".into(),
        EndpointControlError::WebSocket(_) => "websocket".into(),
        EndpointControlError::Json(_) => "json".into(),
        EndpointControlError::MismatchedResponse => "mismatched_response".into(),
        _ => "other_control_error".into(),
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let timeout = Duration::from_secs(8);
    let secret = SecretKey::generate();
    let control = "wss://pab.rgaa.vip/control";
    let registration = register_open_endpoint(
        control,
        &secret,
        OpenRegistrationKind::Device {
            name: "Android metadata compatibility probe".into(),
        },
        tls_connector(None)?,
        timeout,
    )
    .await?;
    let EndpointRegistrationResult::Device {
        tenant_id,
        device_id,
        ..
    } = registration
    else {
        return Err("unexpected registration kind".into());
    };
    let device = DeviceRef {
        tenant_id,
        device_id,
    };
    let config = EndpointControlConfig {
        url: control.into(),
        tenant_id,
        principal: EndpointProofPrincipal::Device { device_id },
        operation_timeout: timeout,
    };
    let mut connection =
        AuthenticatedControlConnection::connect(&config, &secret, tls_connector(None)?).await?;
    let update = DeviceNetworkUpdate {
        schema_version: DEVICE_NETWORK_SCHEMA_VERSION,
        device_ref: device,
        endpoint_key: EndpointKey::new(*secret.public().as_bytes()),
        endpoint_instance_id: EndpointInstanceId::new(),
        address_revision: 1,
        relay_urls: vec!["https://pab-relay.rgaa.vip/".into()],
        direct_addresses: vec![],
        observed_at_unix_ms: now_ms(),
    };
    let network = connection.publish_device_network(&update, timeout).await;
    let network_error = network.as_ref().err().map(category);
    let hello = DeviceHello {
        schema_version: DEVICE_SESSION_SCHEMA_VERSION,
        device_ref: device,
        execution_context: ExecutionContext {
            os_family: OsFamily::Android,
            os_name: "Android".into(),
            os_version: "14".into(),
            architecture: CpuArchitecture::Aarch64,
            execution_scope: ExecutionScope::Native,
            identity: None,
            path_style: PathStyle::Posix,
            interpreter: None,
            cwd: None,
            environment_revision: "android:metadata-compatibility-probe".into(),
        },
        agent_version: env!("CARGO_PKG_VERSION").into(),
        client_version: None,
        observed_at_unix_ms: now_ms(),
    };
    let hello_result = if network.is_ok() {
        Some(connection.publish_device_hello(&hello, timeout).await)
    } else {
        None
    };
    drop(connection);
    let (_updates, receiver) = tokio::sync::watch::channel(update);
    let supervisor = EndpointControlSupervisor::new(
        config,
        secret,
        tls_connector(None)?,
        ReconnectPolicy::default(),
    )?
    .with_device_hello(hello)?
    .with_optional_device_hello()
    .with_device_network(receiver)?
    .spawn();
    let mut status = supervisor.status();
    let optional_connected = tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            if status.borrow().phase == ControlConnectionPhase::Authenticated {
                break;
            }
            status.changed().await?;
        }
        Ok::<(), tokio::sync::watch::error::RecvError>(())
    })
    .await
    .is_ok_and(|value| value.is_ok());
    supervisor.shutdown().await?;
    println!(
        "{}",
        json!({"isolated_protocol_probe":true,"registration":true,"control_authentication":true,
        "network_without_hello":network.is_ok(),"network_error":network_error,
        "android_hello_accepted":hello_result.as_ref().map(Result::is_ok),
        "optional_metadata_authenticated":optional_connected,
        "android_hello_error":hello_result.as_ref().and_then(|result|result.as_ref().err()).map(category)})
    );
    Ok(())
}
