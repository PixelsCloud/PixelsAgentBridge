use std::{env, io::Write, path::PathBuf, sync::Arc, time::Duration};

use pab_bridge::{
    BridgeConfig, BridgeLocalStore, BridgeRuntime, BridgeRuntimeConfig, DevicePasswordProvider,
    DirectoryDevicePasswordProvider,
};
use pab_protocol::DeviceCode;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let code: DeviceCode = env::var("PAB_SMOKE_DEVICE_CODE")?.parse()?;
    let start_file = PathBuf::from(env::var("PAB_SMOKE_START_FILE")?);
    let execute_file = PathBuf::from(env::var("PAB_SMOKE_EXECUTE_FILE")?);
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
    let opened = runtime.open_terminal(device, 80, 24).await?;
    println!("TERMINAL_OPENED {}", opened.session_id);
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
    let active_operators = runtime.current_presence(device).await?;
    if active_operators < 2 {
        return Err(format!("expected two active operators, got {active_operators}").into());
    }
    println!("PRESENCE_OK {active_operators}");
    std::io::stdout().flush()?;
    let mut execute = false;
    for _ in 0..300 {
        if execute_file.exists() {
            execute = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    if !execute {
        return Err("test execute signal was not received".into());
    }
    let marker = env::var("PAB_SMOKE_MARKER")?;
    let suffix = marker
        .strip_prefix("PAB_")
        .ok_or("marker must start with PAB_")?;
    let mut output = Vec::new();
    if opened.shell.contains("powershell") {
        runtime
            .terminal_input(opened.session_id, b"\x1b[1;1R")
            .await?;
        let mut ready = false;
        for _ in 0..150 {
            let next = runtime.terminal_read(opened.session_id).await?;
            output.extend_from_slice(&next.bytes);
            if String::from_utf8_lossy(&output).contains("PS ") {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if !ready {
            return Err(format!(
                "PowerShell prompt did not become ready: {:?}",
                String::from_utf8_lossy(&output)
                    .chars()
                    .take(300)
                    .collect::<String>()
            )
            .into());
        }
    }
    let command = if opened.shell.contains("powershell") {
        format!("Write-Output ('PAB_' + '{suffix}')\r")
    } else {
        format!("printf 'PAB_%s\\n' '{suffix}'\n")
    };
    runtime
        .terminal_input(opened.session_id, command.as_bytes())
        .await?;
    let mut saw_marker = false;
    for _ in 0..100 {
        let next = runtime.terminal_read(opened.session_id).await?;
        output.extend_from_slice(&next.bytes);
        if String::from_utf8_lossy(&output).contains(&marker) {
            saw_marker = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !saw_marker {
        return Err("terminal did not echo its own marker".into());
    }
    println!("TERMINAL_ECHOED");
    std::io::stdout().flush()?;
    runtime.terminal_close(opened.session_id).await?;
    let session_id = opened.session_id.to_string();
    let store = BridgeLocalStore::open(&database).await?;
    let operation = store
        .operations()
        .await?
        .into_iter()
        .find(|operation| operation.id == session_id)
        .ok_or("terminal audit operation is missing")?;
    if operation.kind != "terminal" || operation.state != "completed" {
        return Err("terminal audit operation is incomplete".into());
    }
    let archive = store
        .terminal_bytes(&session_id)
        .await?
        .ok_or("terminal output archive is missing")?;
    if !String::from_utf8_lossy(&archive).contains(&marker) {
        return Err("terminal output archive does not contain the marker".into());
    }
    let events = store.terminal_events(&session_id).await?;
    if !events
        .iter()
        .any(|event| event.kind == "input" && event.state == "completed")
        || !events
            .iter()
            .any(|event| event.kind == "close" && event.state == "completed")
    {
        return Err("terminal input or close audit is missing".into());
    }
    println!("TERMINAL_CLOSED");
    std::io::stdout().flush()?;
    runtime.shutdown().await?;
    Ok(())
}
