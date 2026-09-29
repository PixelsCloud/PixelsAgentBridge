use std::{env, io::Write, path::PathBuf, sync::Arc, time::Duration};

use pab_bridge::{
    BridgeConfig, BridgeLocalStore, BridgeRuntime, BridgeRuntimeConfig, DevicePasswordProvider,
    DirectoryDevicePasswordProvider,
};
use pab_protocol::{DeviceCode, RequestId};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let role = env::var("PAB_SMOKE_ROLE")?;
    if !matches!(role.as_str(), "hold" | "probe") {
        return Err("PAB_SMOKE_ROLE must be hold or probe".into());
    }
    let code: DeviceCode = env::var("PAB_SMOKE_DEVICE_CODE")?.parse()?;
    let source = PathBuf::from(env::var("PAB_SMOKE_SOURCE")?);
    let destination = env::var("PAB_SMOKE_DESTINATION")?;
    let start_file = PathBuf::from(env::var("PAB_SMOKE_START_FILE")?);
    let release_file = env::var_os("PAB_SMOKE_RELEASE_FILE").map(PathBuf::from);
    let database = PathBuf::from(env::var("PAB_BRIDGE_DATABASE")?);
    let password_directory = PathBuf::from(env::var("PAB_DEVICE_PASSWORD_DIR")?);
    let passwords: Arc<dyn DevicePasswordProvider> =
        Arc::new(DirectoryDevicePasswordProvider::new(password_directory));
    let runtime = BridgeRuntime::start(
        BridgeConfig::register_guest_from_env().await?,
        BridgeRuntimeConfig::new(database.clone()),
        passwords,
    )
    .await?;
    let device = runtime.resolve_device_code(code).await?;
    runtime.current_environment(device).await?;
    println!("CONNECTED");
    std::io::stdout().flush()?;
    let mut started = false;
    for _ in 0..300 {
        if start_file.exists() {
            started = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    if !started {
        return Err("test start signal was not received".into());
    }
    let request_id = RequestId::new();
    let result = runtime
        .upload_file_with_id(
            request_id,
            device,
            &source,
            &destination,
            true,
            |offset, size| {
                if role == "hold" && offset == 0 && size > 0 {
                    println!("HOLDING");
                    let _ = std::io::stdout().flush();
                    for _ in 0..200 {
                        if release_file.as_ref().is_some_and(|path| path.exists()) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            },
        )
        .await;
    let remote = runtime.transfer_status(device, request_id).await?;
    let local_store = BridgeLocalStore::open(&database).await?;
    let local = local_store
        .operations()
        .await?
        .into_iter()
        .find(|operation| operation.id == request_id.to_string())
        .ok_or("local transfer audit record is missing")?;
    let outcome = match (
        role.as_str(),
        result,
        remote.state.as_str(),
        local.state.as_str(),
    ) {
        ("hold", Ok(()), "completed", "completed") => {
            println!("UPLOAD_COMPLETE");
            Ok(())
        }
        ("probe", Err(error), "failed", "failed")
            if error
                .to_string()
                .contains("destination already has an active upload")
                && remote.message.as_deref().is_some_and(|message| {
                    message.contains("destination already has an active upload")
                }) =>
        {
            println!("CONFLICT_AUDITED");
            Ok(())
        }
        (_, other, remote_state, local_state) => Err(format!(
            "unexpected upload result: {other:?}; remote={remote_state}; local={local_state}"
        )
        .into()),
    };
    if role == "hold" {
        runtime.shutdown().await?;
    }
    outcome
}
