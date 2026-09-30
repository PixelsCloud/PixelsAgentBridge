#![cfg(windows)]
use pab_protocol::*;
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
#[ignore = "explicit native WM_CLOSE acceptance; build process_fixture and set PAB_PROCESS_TEST_FIXTURE"]
fn hidden_window_graceful_exit_timeout_and_force_only_affect_owned_processes() {
    let path = std::path::PathBuf::from(
        std::env::var_os("PAB_PROCESS_TEST_FIXTURE").expect("dedicated fixture required"),
    );
    assert!(path.is_absolute() && path.is_file());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    for mode in ["close", "ignore-close"] {
        let directory = tempfile::tempdir().unwrap();
        let ready = directory.path().join("ready");
        let mut child = OwnedChild(
            Command::new(&path)
                .arg(mode)
                .arg(&ready)
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(Instant::now() < deadline, "fixture startup timeout");
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid = child.0.id();
        let identity = pab_os_control::process_identity(pid).unwrap();
        let action = |force| SystemQuery::TerminateProcess {
            pid,
            identity: identity.clone(),
            timeout_ms: 100,
            force,
        };
        let data = runtime
            .block_on(pab_os_control::execute(&action(false)))
            .unwrap();
        let SystemQueryData::ProcessTermination { result } = data else {
            panic!("wrong response")
        };
        assert!(result.graceful_supported);
        assert!(!result.forced);
        if mode == "close" {
            assert_eq!(result.outcome, "completed");
            child.0.wait().unwrap();
        } else {
            assert_eq!(result.outcome, "timeout");
            assert!(child.0.try_wait().unwrap().is_none());
            let data = runtime
                .block_on(pab_os_control::execute(&action(true)))
                .unwrap();
            let SystemQueryData::ProcessTermination { result } = data else {
                panic!("wrong response")
            };
            assert_eq!(result.outcome, "completed");
            assert!(result.forced);
            child.0.wait().unwrap();
        }
    }
}
