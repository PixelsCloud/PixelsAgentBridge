#![cfg(windows)]
use pab_os_control::execute;
use pab_protocol::{
    RequestId, ServiceControlAction, ServiceControlResult, SystemQuery, SystemQueryData,
};
use std::time::{Duration, Instant};
use windows_service::{service::*, service_manager::*};
struct Fixture {
    name: String,
    service: Service,
    hang: bool,
    process_identity: std::sync::Mutex<Option<(u32, String)>>,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        let path = std::path::PathBuf::from(
            std::env::var_os("PAB_SERVICE_TEST_FIXTURE")
                .expect("build the service_fixture example and set PAB_SERVICE_TEST_FIXTURE"),
        );
        assert!(path.is_absolute() && path.is_file());
        let name = format!("PabC3Test-{}", RequestId::new());
        let m = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )
        .expect("requires an elevated test environment, never elevate automatically");
        let service = m
            .create_service(
                &ServiceInfo {
                    name: name.clone().into(),
                    display_name: name.clone().into(),
                    service_type: ServiceType::OWN_PROCESS,
                    start_type: ServiceStartType::OnDemand,
                    error_control: ServiceErrorControl::Normal,
                    executable_path: path,
                    launch_arguments: vec![name.clone().into(), mode.into()],
                    dependencies: vec![],
                    account_name: None,
                    account_password: None,
                },
                ServiceAccess::QUERY_STATUS
                    | ServiceAccess::DELETE
                    | ServiceAccess::STOP
                    | ServiceAccess::USER_DEFINED_CONTROL,
            )
            .unwrap();
        Self {
            name,
            service,
            hang: mode == "hang-stop",
            process_identity: std::sync::Mutex::new(None),
        }
    }
    async fn action(&self, control: ServiceControlAction, timeout_ms: u32) -> ServiceControlResult {
        let data = execute(&SystemQuery::ServiceControl {
            name: self.name.clone(),
            control,
            timeout_ms,
        })
        .await
        .unwrap();
        let SystemQueryData::ServiceControl { result } = data else {
            panic!("wrong response")
        };
        if let Some(pid) = result.service.as_ref().and_then(|service| service.pid)
            && let Ok(identity) = pab_os_control::process_identity(pid)
        {
            *self.process_identity.lock().unwrap() = Some((pid, identity));
        }
        result
    }
    fn force_owned_fixture(&self) {
        // Retain the originally observed creation identity. Never capture a new identity during cleanup.
        if let Some((pid, identity)) = self.process_identity.lock().unwrap().as_ref() {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let _ = runtime.block_on(execute(&SystemQuery::TerminateProcess {
                pid: *pid,
                identity: identity.clone(),
                timeout_ms: 100,
                force: true,
            }));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.hang {
            let _ = self.service.notify(UserEventCode::from_raw(128).unwrap());
        } else {
            self.force_owned_fixture();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.service.query_status() {
                Ok(s) if s.current_state != ServiceState::Stopped && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => break,
            }
        }
        if self
            .service
            .query_status()
            .is_ok_and(|s| s.current_state != ServiceState::Stopped)
        {
            self.force_owned_fixture();
        }
        self.service
            .delete()
            .expect("delete only the owned fixture service");
    }
}
#[test]
#[ignore = "explicit native SCM acceptance with a dedicated service fixture; requires service creation rights"]
fn dedicated_scm_service_covers_lifecycle_config_failure_and_timeout() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    {
        let fixture = Fixture::new("normal");
        runtime.block_on(async {
            assert_eq!(
                fixture
                    .action(ServiceControlAction::Enable, 5000)
                    .await
                    .outcome,
                "completed"
            );
            let r = fixture.action(ServiceControlAction::Disable, 5000).await;
            assert_eq!(r.outcome, "completed");
            assert_eq!(r.service.unwrap().state, "stopped");
            assert_eq!(
                fixture
                    .action(ServiceControlAction::Start, 5000)
                    .await
                    .outcome,
                "failed"
            );
            assert_eq!(
                fixture
                    .action(ServiceControlAction::Enable, 5000)
                    .await
                    .outcome,
                "completed"
            );
            let start = fixture.action(ServiceControlAction::Start, 5000).await;
            assert_eq!(start.outcome, "completed", "{start:?}");
            let info = start.service.unwrap();
            assert_eq!(info.state, "running");
            assert!(info.executable.unwrap().ends_with("service_fixture.exe"));
            assert!(info.errors.is_empty());
            let restart = fixture.action(ServiceControlAction::Restart, 5000).await;
            assert_eq!(restart.outcome, "completed", "{restart:?}");
            assert_eq!(
                fixture
                    .action(ServiceControlAction::Stop, 5000)
                    .await
                    .outcome,
                "completed"
            );
            assert!(
                !fixture
                    .action(ServiceControlAction::Stop, 5000)
                    .await
                    .changed
            );
        });
    }
    {
        let fixture = Fixture::new("hang-stop");
        runtime.block_on(async {
            assert_eq!(
                fixture
                    .action(ServiceControlAction::Start, 5000)
                    .await
                    .outcome,
                "completed"
            );
            let r = fixture.action(ServiceControlAction::Stop, 100).await;
            assert_eq!(r.outcome, "timeout", "{r:?}");
            assert!(r.changed);
            assert_eq!(r.service.unwrap().state, "running");
        });
    }
    {
        let fixture = Fixture::new("fail-start");
        runtime.block_on(async {
            let r = fixture.action(ServiceControlAction::Start, 5000).await;
            assert_eq!(r.outcome, "failed", "{r:?}");
            assert_eq!(r.service.unwrap().exit_code, Some(5));
        });
    }
}
