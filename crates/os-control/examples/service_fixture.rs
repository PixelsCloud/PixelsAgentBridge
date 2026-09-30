//! Dedicated test service only. Never included in production binaries/installers.
#[cfg(not(windows))]
fn main() {}
#[cfg(windows)]
static MODE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
#[cfg(windows)]
fn main() -> windows_service::Result<()> {
    let name = std::env::args()
        .nth(1)
        .expect("unique fixture service name required");
    MODE.set(std::env::args().nth(2).unwrap_or_default())
        .unwrap();
    windows_service::service_dispatcher::start(name, ffi_service_main)
}
#[cfg(windows)]
windows_service::define_windows_service!(ffi_service_main, service_main);
#[cfg(windows)]
fn service_main(args: Vec<std::ffi::OsString>) {
    use std::time::Duration;
    use windows_service::{
        service::*,
        service_control_handler::{self, ServiceControlHandlerResult},
    };
    let name = args
        .first()
        .expect("service name")
        .to_string_lossy()
        .to_string();
    let mode = MODE.get().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    // windows-service releases its handler closure after Stop; retain the sender so a hung-stop fixture remains alive.
    let _keep_sender = send.clone();
    let hang = mode == "hang-stop";
    let Ok(handle) = service_control_handler::register(&name, move |control| match control {
        ServiceControl::Stop if !hang => {
            let _ = send.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Stop | ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        ServiceControl::UserEvent(code) if code.to_raw() == 128 => {
            let _ = send.send(());
            ServiceControlHandlerResult::NoError
        }
        _ => ServiceControlHandlerResult::NotImplemented,
    }) else {
        return;
    };
    let mut status = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::StartPending,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 1,
        wait_hint: Duration::from_secs(1),
        process_id: None,
    };
    let _ = handle.set_service_status(status.clone());
    if mode == "fail-start" {
        status.current_state = ServiceState::Stopped;
        status.exit_code = ServiceExitCode::Win32(5);
        status.checkpoint = 0;
        let _ = handle.set_service_status(status);
        return;
    }
    status.current_state = ServiceState::Running;
    status.controls_accepted = ServiceControlAccept::STOP;
    status.checkpoint = 0;
    let _ = handle.set_service_status(status.clone());
    let _ = receive.recv_timeout(Duration::from_secs(120));
    status.current_state = ServiceState::Stopped;
    status.controls_accepted = ServiceControlAccept::empty();
    let _ = handle.set_service_status(status);
}
