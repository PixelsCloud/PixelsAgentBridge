use std::{ffi::OsString, future::Future, pin::Pin, sync::OnceLock, time::Duration};
use tokio::sync::watch;
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

pub type ServiceFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'static>>;
struct Launch {
    name: String,
    entry: fn(watch::Receiver<bool>) -> ServiceFuture,
}
static LAUNCH: OnceLock<Launch> = OnceLock::new();

pub fn run(name: String, entry: fn(watch::Receiver<bool>) -> ServiceFuture) -> Result<(), String> {
    LAUNCH
        .set(Launch {
            name: name.clone(),
            entry,
        })
        .map_err(|_| "service dispatcher already started")?;
    service_dispatcher::start(name, ffi_main).map_err(|e| e.to_string())
}
define_windows_service!(ffi_main, service_main);
fn status(state: ServiceState, code: u32) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(code),
        checkpoint: if state == ServiceState::StopPending {
            1
        } else {
            0
        },
        wait_hint: Duration::from_secs(10),
        process_id: None,
    }
}
fn service_main(_: Vec<OsString>) {
    let launch = LAUNCH.get().expect("registered service launch");
    let (stop, mut observed) = watch::channel(false);
    let Ok(handler) =
        service_control_handler::register(&launch.name, move |control| match control {
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            ServiceControl::Stop | ServiceControl::Shutdown => {
                stop.send_replace(true);
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        })
    else {
        return;
    };
    let _ = handler.set_service_status(status(ServiceState::StartPending, 0));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build();
    let result = match runtime {
        Err(error) => Err(error.to_string()),
        Ok(runtime) => {
            let _ = handler.set_service_status(status(ServiceState::Running, 0));
            runtime.block_on(async {
                let future=(launch.entry)(observed.clone());tokio::pin!(future);
                tokio::select! {
                    result=&mut future=>result,
                    _=async {while !*observed.borrow_and_update(){if observed.changed().await.is_err(){break;}}}=>{
                        let _=handler.set_service_status(status(ServiceState::StopPending,0));
                        tokio::time::timeout(Duration::from_secs(9),future).await.map_err(|_|"service stop deadline exceeded".to_string())?
                    }
                }
            })
        }
    };
    if let Err(error) = &result {
        eprintln!("Pixels server service stopped: {error}");
    }
    let _ = handler.set_service_status(status(ServiceState::Stopped, u32::from(result.is_err())));
}
