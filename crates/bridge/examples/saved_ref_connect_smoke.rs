//! Live regression check: reconnect a saved DeviceRef without resolving its code.
use std::{
    env,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use pab_bridge::{
    BridgeConfig, BridgeRuntime, BridgeRuntimeConfig, DeviceConnectionPhase,
    DirectoryDevicePasswordProvider, RuntimeEventKind,
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
    let retries = Arc::new(AtomicUsize::new(0));
    let observed_retries = retries.clone();
    let mut events = runtime.subscribe();
    let observer = tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            if matches!(
                event.kind,
                RuntimeEventKind::BridgeRetrying { .. }
                    | RuntimeEventKind::DeviceConnection {
                        phase: DeviceConnectionPhase::Retrying,
                        ..
                    }
            ) {
                observed_retries.fetch_add(1, Ordering::Relaxed);
            }
        }
    });
    let result = tokio::time::timeout(Duration::from_secs(60), async {
        for attempt in 1..=2 {
            let started = Instant::now();
            let context = runtime.connect_device(device).await?;
            assert_eq!(context.device_ref, device);
            // Repeated operations reuse the authenticated session.
            assert_eq!(
                runtime.current_environment(device).await?.device_ref,
                device
            );
            println!(
                "saved_ref_connect_{attempt}=ok os={:?} seconds={:.2}",
                context.execution.os_family,
                started.elapsed().as_secs_f64()
            );
            runtime.disconnect_device(device).await;
        }
        Ok::<_, pab_bridge::RuntimeError>(())
    })
    .await;
    runtime.shutdown().await?;
    observer.abort();
    let _ = observer.await;
    result??;
    let count = retries.load(Ordering::Relaxed);
    println!("connection_retries={count}");
    if env::var_os("PAB_REQUIRE_FIRST_ATTEMPT").is_some() {
        assert_eq!(
            count, 0,
            "fresh Relay connections must succeed without retrying"
        );
    }
    Ok(())
}
