//! Live regression check: reconnect a saved DeviceRef without resolving its code.
use std::{env, path::PathBuf, sync::Arc, time::Duration};

use pab_bridge::{
    BridgeConfig, BridgeRuntime, BridgeRuntimeConfig, DirectoryDevicePasswordProvider,
};
use pab_protocol::DeviceRef;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let device: DeviceRef = serde_json::from_str(&env::var("PAB_SMOKE_DEVICE_REF")?)?;
    let runtime = BridgeRuntime::start(
        BridgeConfig::register_guest_from_env().await?,
        BridgeRuntimeConfig::new(PathBuf::from(env::var("PAB_BRIDGE_DATABASE")?)),
        Arc::new(DirectoryDevicePasswordProvider::new(PathBuf::from(
            env::var("PAB_DEVICE_PASSWORD_DIR")?,
        ))),
    )
    .await?;
    let result = tokio::time::timeout(Duration::from_secs(60), async {
        for attempt in 1..=2 {
            let context = runtime.connect_device(device).await?;
            assert_eq!(context.device_ref, device);
            // Repeated operations reuse the authenticated session.
            assert_eq!(
                runtime.current_environment(device).await?.device_ref,
                device
            );
            println!(
                "saved_ref_connect_{attempt}=ok os={:?}",
                context.execution.os_family
            );
            runtime.disconnect_device(device).await;
        }
        Ok::<_, pab_bridge::RuntimeError>(())
    })
    .await;
    runtime.shutdown().await?;
    result??;
    Ok(())
}
