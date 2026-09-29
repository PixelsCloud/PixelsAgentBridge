use std::{env, path::PathBuf, sync::Arc, time::Duration};

use pab_bridge::{
    BridgeConfig, BridgeLocalStore, BridgeRuntime, BridgeRuntimeConfig, DevicePasswordProvider,
    DirectoryDevicePasswordProvider,
};
use pab_protocol::{DeviceCode, RequestId};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let code: DeviceCode = env::var("PAB_SMOKE_DEVICE_CODE")?.parse()?;
    let source = PathBuf::from(env::var("PAB_SMOKE_SOURCE")?);
    let destination = env::var("PAB_SMOKE_DESTINATION")?;
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
    let id = RequestId::new();
    runtime
        .prepare_transfer_record(
            id,
            device,
            "upload",
            &source.to_string_lossy(),
            &destination,
            false,
        )
        .await?;
    let (sender, mut receiver) = mpsc::unbounded_channel();
    {
        let upload = runtime.upload_file_with_id(
            id,
            device,
            &source,
            &destination,
            false,
            move |offset, size| {
                let _ = sender.send((offset, size));
            },
        );
        tokio::pin!(upload);
        loop {
            tokio::select! {
                result = &mut upload => return Err(format!("upload finished before cancellation: {result:?}").into()),
                progress = receiver.recv() => {
                    if progress.is_some_and(|(offset, _)| offset > 0) {
                        break;
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(45)) => {
                    return Err("upload made no progress".into());
                }
            }
        }
    }
    if !runtime.cancel_transfer_record(id).await? {
        return Err("cancellation was not recorded".into());
    }
    let store = BridgeLocalStore::open(&database).await?;
    let record = store
        .operations()
        .await?
        .into_iter()
        .find(|record| record.id == id.to_string())
        .ok_or("transfer audit record missing")?;
    if record.state != "cancel_requested" || record.finished_at_unix_ms.is_some() {
        return Err(format!("unexpected local state: {}", record.state).into());
    }
    let remote =
        tokio::time::timeout(Duration::from_secs(20), runtime.transfer_status(device, id)).await;
    let remote_state = match remote {
        Ok(Ok(snapshot)) => snapshot.state,
        Ok(Err(_)) => "unavailable".to_owned(),
        Err(_) => "timeout".to_owned(),
    };
    let final_state = if remote_state == "failed" {
        tokio::time::timeout(Duration::from_secs(40), async {
            loop {
                let state = store
                    .operations()
                    .await?
                    .into_iter()
                    .find(|record| record.id == id.to_string())
                    .ok_or("transfer audit record missing")?
                    .state;
                if state == "failed" {
                    break Ok::<_, Box<dyn std::error::Error>>(state);
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
        .await??
    } else {
        record.state
    };
    println!("request={id} local={final_state} remote={remote_state}");
    runtime.shutdown().await?;
    Ok(())
}
