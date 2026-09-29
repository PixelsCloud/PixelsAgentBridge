use std::{
    ffi::OsString,
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use serde::Deserialize;
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

const SERVICE_NAME: &str = "PixelsAgentBridgeExecutor";

#[derive(Deserialize)]
struct InstalledSettings {
    deployment_id: String,
    control_url: String,
    relay_urls: String,
}

pub fn run() -> Result<(), String> {
    load_installed_settings()?;
    service_dispatcher::start(SERVICE_NAME, ffi_service_main).map_err(|error| error.to_string())
}

fn load_installed_settings() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let root = executable
        .parent()
        .ok_or("Executor has no installation directory")?;
    let bytes = fs::read(root.join("settings.json")).map_err(|error| error.to_string())?;
    let settings = parse_installed_settings(&bytes)?;
    let data_root = std::env::var_os("PROGRAMDATA")
        .ok_or("PROGRAMDATA is unavailable")
        .map(std::path::PathBuf::from)?
        .join("PixelsAgentBridge");
    // SAFETY: This runs before the service dispatcher starts any application threads.
    unsafe {
        std::env::set_var("PAB_DATA_DIR", data_root);
        std::env::set_var("PAB_DEPLOYMENT_ID", settings.deployment_id);
        std::env::set_var("PAB_CONTROL_URL", settings.control_url);
        std::env::set_var("PAB_RELAY_URLS", settings.relay_urls);
    }
    Ok(())
}

fn parse_installed_settings(bytes: &[u8]) -> Result<InstalledSettings, String> {
    let json = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    serde_json::from_slice(json).map_err(|error| error.to_string())
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_arguments: Vec<OsString>) {
    if let Err(error) = run_service() {
        tracing::error!(%error, "Executor service stopped with an error");
    }
}

fn status(state: ServiceState, controls: ServiceControlAccept, exit_code: u32) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: controls,
        exit_code: ServiceExitCode::Win32(exit_code),
        checkpoint: 0,
        wait_hint: Duration::from_secs(10),
        process_id: None,
    }
}

fn run_service() -> Result<(), String> {
    let stopping = Arc::new(AtomicBool::new(false));
    let handler_stopping = stopping.clone();
    let handler = service_control_handler::register(SERVICE_NAME, move |control| match control {
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        ServiceControl::Stop | ServiceControl::Shutdown => {
            handler_stopping.store(true, Ordering::Release);
            ServiceControlHandlerResult::NoError
        }
        _ => ServiceControlHandlerResult::NotImplemented,
    })
    .map_err(|error| error.to_string())?;
    handler
        .set_service_status(status(
            ServiceState::StartPending,
            ServiceControlAccept::empty(),
            0,
        ))
        .map_err(|error| error.to_string())?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    handler
        .set_service_status(status(
            ServiceState::Running,
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            0,
        ))
        .map_err(|error| error.to_string())?;
    pab_executor::windows_sas::set_service_context(true);
    let result = runtime.block_on(async {
        let root = pab_agent_core::DataPaths::for_scope(pab_agent_core::DataScope::Machine)
            .map_err(|error| error.to_string())?;
        let local_service = pab_executor::local_ipc::spawn_local_service(root.root().to_path_buf());
        let result = tokio::select! {
            result = async {
                let config = pab_executor::bootstrapped_config().await.map_err(|error| error.to_string())?;
                pab_executor::run_executor(config).await.map_err(|error| error.to_string())
            } => result,
            _ = async {
                while !stopping.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            } => Ok(()),
        };
        local_service.abort();
        result
    });
    pab_executor::windows_sas::set_service_context(false);
    handler
        .set_service_status(status(
            ServiceState::Stopped,
            ServiceControlAccept::empty(),
            u32::from(result.is_err()),
        ))
        .map_err(|error| error.to_string())?;
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn accepts_settings_written_by_windows_powershell() {
        let bytes = b"\xef\xbb\xbf{\"deployment_id\":\"id\",\"control_url\":\"wss://example.test\",\"relay_urls\":\"https://example.test\"}";
        let settings = super::parse_installed_settings(bytes).unwrap();
        assert_eq!(settings.deployment_id, "id");
    }
}
