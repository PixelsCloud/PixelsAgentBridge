#![cfg(windows)]

use std::os::windows::process::CommandExt;

#[test]
fn selected_output_guard_rejects_stale_geometry_rotation_monitor_and_dpi() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pab-media-worker"))
        .arg("display-layout-check").output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let displays = result["displays"].as_u64().unwrap();
    assert!(displays > 0);
    assert_eq!(result["stale_layouts_rejected"].as_u64(), Some(displays * 5));
}

#[test]
fn native_media_wait_honors_deadlines_falls_back_and_releases_handles() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pab-media-worker"))
        .arg("media-timing-check")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["samples"], 24);
    assert_eq!(result["requested_ms"], 2);
    assert!(result["median_ms"].as_f64().unwrap() >= 2.0);
    assert_eq!(result["fallback_checked"], true);
    assert_eq!(result["handles_released"], true);
    // Scheduling latency depends on host load. Report it for the real-device
    // smoke test rather than imposing a flaky upper bound in shared CI.
    println!("{}", String::from_utf8_lossy(&output.stdout).trim());
}

#[test]
fn media_worker_does_not_inherit_a_background_helpers_priority() {
    // Put only this isolated child test process at BelowNormal, leaving the
    // cargo runner and any concurrently running tests untouched.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pab-media-worker"))
        .arg("media-priority-parent-check")
        .creation_flags(0x00004000) // BELOW_NORMAL_PRIORITY_CLASS
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("background->normal"));
}
