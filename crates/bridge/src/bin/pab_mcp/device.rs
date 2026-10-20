//! User-facing pairing entry. Passwords travel over stdin, never through model tool arguments.
use std::{
    io::{IsTerminal, Read},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use pab_agent_core::DataPaths;
use pab_bridge::{
    BridgeLocalStore, BridgeRuntime, BridgeRuntimeConfig, MemoryDevicePasswordProvider,
    RememberedDevice,
};
use pab_protocol::{DeviceCode, MAX_DEVICE_PASSWORD_BYTES};
use zeroize::Zeroizing;

pub(super) async fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() != 3 || args[0] != "connect" || args[2] != "--password-stdin" {
        return Err("usage: pab-mcp device connect <9-digit-code> --password-stdin".into());
    }
    let code: DeviceCode = args[1].parse()?;
    if std::io::stdin().is_terminal() {
        return Err("pipe the device password on stdin; do not use a command-line password".into());
    }
    let mut password = Zeroizing::new(String::new());
    std::io::stdin()
        .take(MAX_DEVICE_PASSWORD_BYTES as u64 + 3)
        .read_to_string(&mut password)?;
    while password.ends_with(['\r', '\n']) {
        password.pop();
    }
    if password.is_empty() || password.len() > MAX_DEVICE_PASSWORD_BYTES {
        return Err("invalid device password length".into());
    }
    let config = super::endpoint_config().await?;
    let paths = DataPaths::for_operator()?;
    let database = std::env::var_os("PAB_BRIDGE_DATABASE")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.bridge_database());
    let local = BridgeLocalStore::open(&database).await?;
    let passwords = Arc::new(MemoryDevicePasswordProvider::default());
    let mut options = BridgeRuntimeConfig::new(database);
    options.resume_incomplete_on_start = false;
    let runtime = BridgeRuntime::start(config, options, passwords.clone()).await?;
    let result = tokio::time::timeout(Duration::from_secs(60), async {
        let device = runtime.resolve_device_code(code).await.map_err(|e| e.to_string())?;
        passwords.set_password(device.device_id, password.to_string()).map_err(|e| e.to_string())?;
        let target = runtime.connect_device(device).await.map_err(|e| e.to_string())?;
        // As in Desktop: verify the actual device first, then remember it and save credentials.
        runtime.remember_device(&RememberedDevice { device_ref: device, code, alias: String::new(),
            os_family: Some(target.execution.os_family), os_reminder: target.compact_reminder() }).await.map_err(|e| e.to_string())?;
        local.save_device_password(device.device_id, &password).await.map_err(|e| e.to_string())?;
        Ok::<_, String>(serde_json::json!({"connected":true,"device_code":code,"device_ref":device,"target":target}))
    }).await;
    let shutdown = runtime.shutdown().await;
    let result = result.map_err(|_| "device pairing timed out")??;
    shutdown?;
    println!("{}", result);
    Ok(())
}
