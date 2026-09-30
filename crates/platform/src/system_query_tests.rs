use super::*;

#[test]
fn native_info_has_cpu_memory_identity_and_cpu_sampling_is_explicit() {
    let mut c = SystemCollector::default();
    let id = RequestId::new();
    let reply = c.query(
        id,
        &SystemQuery::Info {
            include_gpu: false,
            sample_cpu: true,
        },
    );
    assert_eq!(reply.state, "completed", "{reply:?}");
    assert!(
        reply.cpu_sample_ms.unwrap() >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.as_millis() as u64
    );
    assert!(reply.sampled_at_unix_ms >= reply.sampled_from_unix_ms);
    let Some(SystemQueryData::Info { info }) = reply.data else {
        panic!()
    };
    assert!(info.logical_cores > 0);
    assert!(info.memory_total_bytes > 0);
    assert!(info.memory_available_bytes <= info.memory_total_bytes);
    assert_eq!(info.executor.pid, std::process::id());
    assert_eq!(info.gpu.status, "not_requested");
    assert!(info.cpu_usage_basis_points.is_some());
}

#[test]
fn pid_filter_stays_exact_after_a_previous_full_collection() {
    let mut c = SystemCollector::default();
    let _ = c.query(
        RequestId::new(),
        &SystemQuery::Processes {
            pid: None,
            name: None,
            user: None,
            limit: 1000,
            sample_cpu: false,
        },
    );
    let r = c.query(
        RequestId::new(),
        &SystemQuery::Processes {
            pid: Some(std::process::id()),
            name: None,
            user: None,
            limit: 100,
            sample_cpu: false,
        },
    );
    let Some(SystemQueryData::Processes { entries }) = r.data else {
        panic!()
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].pid, std::process::id());
    assert_eq!(entries[0].cpu_usage_basis_points, None);
    let r = c.query(
        RequestId::new(),
        &SystemQuery::Processes {
            pid: Some(std::process::id()),
            name: Some("does-not-match-fixture".into()),
            user: None,
            limit: 100,
            sample_cpu: false,
        },
    );
    assert_eq!(r.returned_count, 0);
    assert!(!r.truncated);
    assert_eq!(
        c.query(
            RequestId::new(),
            &SystemQuery::Process {
                pid: u32::MAX,
                sample_cpu: false
            }
        )
        .state,
        "failed"
    );
}

#[test]
fn disks_interfaces_and_limits_return_bounded_typed_results() {
    let mut c = SystemCollector::default();
    for q in [
        SystemQuery::Disks { limit: 1 },
        SystemQuery::Networks {
            name: None,
            limit: 1,
        },
        SystemQuery::Processes {
            pid: None,
            name: None,
            user: None,
            limit: 1,
            sample_cpu: false,
        },
    ] {
        let r = c.query(RequestId::new(), &q);
        assert_eq!(r.state, "completed", "{r:?}");
        assert!(r.returned_count <= 1);
        assert!(serde_json::to_vec(&r).unwrap().len() <= MAX_SYSTEM_REPLY_BYTES);
        if let Some(SystemQueryData::Disks { entries }) = r.data {
            for d in entries {
                assert!(d.available_bytes <= d.total_bytes);
            }
        }
    }
}

#[test]
fn gpu_query_has_independent_status_and_keeps_base_information() {
    let mut c = SystemCollector::default();
    let r = c.query(
        RequestId::new(),
        &SystemQuery::Info {
            include_gpu: true,
            sample_cpu: false,
        },
    );
    assert_eq!(r.state, "completed", "{r:?}");
    let Some(SystemQueryData::Info { info }) = r.data else {
        panic!()
    };
    assert!(info.memory_total_bytes > 0);
    assert!(matches!(
        info.gpu.status.as_str(),
        "available" | "partial" | "unavailable" | "unsupported"
    ));
    assert_eq!(info.gpu.backend, "nvml");
    // No GPU presence is inferred from the library/driver being unavailable.
    if info.gpu.status == "unavailable" {
        assert!(!info.gpu.errors.is_empty());
    }
}

#[test]
fn escaped_multibyte_output_is_bounded_without_invalid_unicode_or_fake_cpu() {
    assert_eq!(basis_points(f32::NAN), None);
    assert_eq!(basis_points(f32::INFINITY), None);
    assert_eq!(basis_points(250.), Some(25000));
    assert_eq!(bounded("中文", 4), "中");
    let mut r = SystemQueryReply::pending(RequestId::new(), &SystemQuery::Disks { limit: 1000 });
    r.state = "completed".into();
    let mut entries = Vec::new();
    let mut bytes = 0;
    for _ in 0..1000 {
        let item = DiskInfo {
            name: "\"\t中文".repeat(100),
            mount_point: "/tmp".into(),
            filesystem: "fixture".into(),
            kind: "unknown".into(),
            total_bytes: 1,
            available_bytes: 1,
            removable: false,
            readonly: false,
        };
        if !push_bounded(&mut entries, item, &mut bytes, &mut r) {
            break;
        }
    }
    r.data = Some(SystemQueryData::Disks { entries });
    bound_reply(&mut r);
    assert!(r.truncated);
    assert_eq!(r.stop_reason.as_deref(), Some("output_bytes_limit"));
    assert!(r.returned_count < 1000);
    assert!(serde_json::to_vec(&r).unwrap().len() <= MAX_SYSTEM_REPLY_BYTES);
}

struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn process_appearance_and_exit_are_observed_without_stale_pid_results() {
    let mut c = SystemCollector::default();
    let mut child = ChildGuard(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "system_query::tests::owned_child_fixture",
            ])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let pid = child.0.id();
    let q = SystemQuery::Process {
        pid,
        sample_cpu: false,
    };
    let r = c.query(RequestId::new(), &q);
    assert_eq!(r.state, "completed");
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    assert_eq!(c.query(RequestId::new(), &q).state, "failed");
}
#[test]
#[ignore = "spawned exclusively by the owned-child lifecycle test"]
fn owned_child_fixture() {
    std::thread::sleep(Duration::from_secs(30));
}
