#[test]
#[cfg(windows)]
fn native_release_state_survives_interruption_and_excludes_concurrent_owners() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pab-media-worker"))
        .arg("input-recovery-check")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
