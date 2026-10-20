//! Explicit live-session check; run as the Linux graphical user, not root.
#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use pab_os_control::execution::{LinuxGraphicalSession, current_identity, process_user_identity};
    let pid = std::process::id();
    let native = process_user_identity(pid)?;
    assert_eq!(native, current_identity()?);
    assert!(native.session_id.is_none());
    let uid: u32 = native.account_id.strip_prefix("uid:").ok_or("invalid UID")?.parse()?;
    let session = LinuxGraphicalSession::for_process(pid, uid)?;
    assert!(session.current(pid));
    assert!(LinuxGraphicalSession::for_process(pid, 0).is_err());
    assert!(LinuxGraphicalSession::for_process(u32::MAX, uid).is_err());
    println!("{}", serde_json::json!({"passed":true,"uid":uid,"graphical":format!("{session:?}"),"numeric_session_invented":false}));
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn main() { eprintln!("This check requires Linux"); std::process::exit(1); }
