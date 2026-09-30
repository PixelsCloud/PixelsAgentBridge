use super::*;
use crate::SystemCollector;
use std::process::{Child, Command, Stdio};
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[tokio::test]
async fn native_identity_mismatch_does_not_kill_and_explicit_force_confirms_exit() {
    let mut child = OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "system_query::tests::owned_child_fixture",
            ])
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let pid = child.0.id();
    let mut c = SystemCollector::default();
    let snapshot = c.query(
        RequestId::new(),
        &SystemQuery::Process {
            pid,
            sample_cpu: false,
        },
    );
    let Some(SystemQueryData::Process { process }) = snapshot.data else {
        panic!("{snapshot:?}");
    };
    let identity = process.termination_identity.unwrap();
    let wrong = SystemQuery::TerminateProcess {
        pid,
        identity: format!("{identity}-wrong"),
        timeout_ms: 100,
        force: true,
    };
    let reply = query_async(RequestId::new(), &wrong).await.unwrap();
    assert_eq!(reply.state, "failed");
    assert!(child.0.try_wait().unwrap().is_none());
    #[cfg(windows)]
    {
        let graceful = query_async(
            RequestId::new(),
            &SystemQuery::TerminateProcess {
                pid,
                identity: identity.clone(),
                timeout_ms: 100,
                force: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(graceful.state, "failed");
        assert!(
            matches!(graceful.data,Some(SystemQueryData::ProcessTermination{result}) if result.outcome=="unsupported_graceful" && !result.forced)
        );
        assert!(child.0.try_wait().unwrap().is_none());
    }
    let reply = query_async(
        RequestId::new(),
        &SystemQuery::TerminateProcess {
            pid,
            identity,
            timeout_ms: 100,
            force: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(reply.state, "completed", "{reply:?}");
    child.0.wait().unwrap();
    assert_eq!(
        c.query(
            RequestId::new(),
            &SystemQuery::Process {
                pid,
                sample_cpu: false
            }
        )
        .state,
        "failed"
    );
}
#[tokio::test]
async fn self_protection_missing_process_and_service_inventory_are_explicit() {
    let identity = pab_os_control::process_identity(std::process::id()).unwrap();
    let q = SystemQuery::TerminateProcess {
        pid: std::process::id(),
        identity,
        timeout_ms: 100,
        force: true,
    };
    let r = query_async(RequestId::new(), &q).await.unwrap();
    assert_eq!(r.state, "failed");
    assert!(r.error.unwrap().contains("protected PID"));
    let r = query_async(
        RequestId::new(),
        &SystemQuery::Services {
            name: Some("pab-impossible-test-service-unique-20261001".into()),
            state: None,
            limit: 1,
        },
    )
    .await
    .unwrap();
    assert_eq!(r.state, "completed", "{r:?}");
    assert_eq!(r.returned_count, 0);
    for q in [
        SystemQuery::Service {
            name: "pab-impossible-test-service-unique-20261001".into(),
        },
        SystemQuery::ServiceControl {
            name: "pab-impossible-test-service-unique-20261001".into(),
            control: ServiceControlAction::Start,
            timeout_ms: 100,
        },
    ] {
        let r = query_async(RequestId::new(), &q).await.unwrap();
        assert_eq!(r.state, "failed", "{r:?}");
    }
}
