//! Runs the desktop's actual deadline policy in a separate process. Fixture
//! modes are test-only; no fault-injection switch is added to the shipped app.
#![cfg(windows)]

#[path = "../../../apps/desktop/src-tauri/src/application_deadline.rs"]
mod deadline;

use std::{
    fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn fixture(name: &str, root: &Path) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--ignored", "--nocapture"])
        .env("PAB_APPLICATION_DEADLINE_FIXTURE", root)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn fixture_root() -> PathBuf {
    let root = PathBuf::from(std::env::var_os("PAB_APPLICATION_DEADLINE_FIXTURE").unwrap());
    assert!(root.is_absolute() && !root.is_symlink());
    assert!(
        root.file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("pab-app-deadline-")
    );
    root
}

#[test]
#[ignore = "child process fixture; invoked only by the deadline acceptance test"]
fn owned_application_fixture() {
    let root = fixture_root();
    let started = Instant::now();
    let mut tick = 0u64;
    while !root.join("stop").exists() && started.elapsed() < Duration::from_secs(70) {
        // Publishing with a replace is unnecessary: the reader retries while
        // this tiny test heartbeat is being written.
        fs::write(root.join("heartbeat"), tick.to_string()).unwrap();
        tick += 1;
        thread::sleep(Duration::from_millis(100));
    }
    fs::write(root.join("application-exited"), b"normal").unwrap();
}

#[test]
#[ignore = "child process fixture; invoked only by the deadline acceptance test"]
fn blocked_helper_fixture() {
    let root = fixture_root();
    let mut app = fixture("owned_application_fixture", &root);
    let started = Instant::now();
    while !root.join("heartbeat").exists() {
        assert!(app.try_wait().unwrap().is_none());
        assert!(started.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(20));
    }
    fs::write(root.join("helper-dispatched"), b"once").unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        deadline::run(pab_protocol::RequestId::new(), || {
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn Sleep(milliseconds: u32);
            }
            // Native, non-cancellable call on the real blocking thread. Only
            // this isolated helper is allowed to be terminated by the policy.
            unsafe { Sleep(u32::MAX) };
        })
        .await
        .unwrap();
    });
    panic!("the blocked helper must retire instead of returning");
}

struct Cleanup {
    root: PathBuf,
    helper: Child,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::write(self.root.join("stop"), b"stop");
        let _ = self.helper.kill();
        let _ = self.helper.wait();
        for _ in 0..100 {
            if self.root.join("application-exited").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
#[ignore = "native 20-second process retirement acceptance"]
fn blocked_native_call_retires_only_helper_and_keeps_application_alive() {
    let directory = tempfile::Builder::new()
        .prefix("pab-app-deadline-")
        .tempdir()
        .unwrap();
    let root = directory.path();
    let mut cleanup = Cleanup {
        root: root.to_owned(),
        helper: fixture("blocked_helper_fixture", root),
    };
    let started = Instant::now();
    let status = loop {
        if let Some(status) = cleanup.helper.try_wait().unwrap() {
            break status;
        }
        assert!(
            started.elapsed() < Duration::from_secs(35),
            "helper did not retire"
        );
        thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(status.code(), Some(1));
    assert!(started.elapsed() >= Duration::from_secs(19));
    assert_eq!(fs::read(root.join("helper-dispatched")).unwrap(), b"once");
    assert!(!root.join("application-exited").exists());
    let tick = || {
        fs::read_to_string(root.join("heartbeat"))
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
    };
    let before = tick().unwrap_or(0);
    thread::sleep(Duration::from_millis(400));
    assert!(
        tick().is_some_and(|after| after > before),
        "owned app stopped with helper"
    );
    drop(cleanup);
    assert_eq!(
        fs::read(root.join("application-exited")).unwrap(),
        b"normal"
    );
    println!(
        "APPLICATION_DEADLINE helper_exit=1 deadline_20s=verified app_survived=true app_stop=normal"
    );
}

#[tokio::test]
async fn completed_and_panicked_calls_return_without_process_retirement() {
    assert_eq!(
        deadline::run(pab_protocol::RequestId::new(), || 42)
            .await
            .unwrap(),
        42
    );
    assert!(
        deadline::run(pab_protocol::RequestId::new(), || panic!(
            "owned fixture panic"
        ))
        .await
        .is_err()
    );
}
