//! Android boundary. Existing desktop execution paths do not depend on this crate.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
mod ocr;

#[cfg(any(target_os = "android", test))]
mod activity;
mod broker;
pub mod config;
pub mod control;
mod documents;
mod execution;
mod filesystem;
mod peer;
mod port_forward;
mod screenshot;
mod storage;
mod system;

/// Describe the shared libraries loaded in the application, not network readiness.
pub fn shared_core_probe() -> String {
    let policy = pab_agent_core::ReconnectPolicy::default();
    serde_json::json!({
        "status": "loaded",
        "architecture": std::env::consts::ARCH,
        "os": std::env::consts::OS,
        "version": env!("CARGO_PKG_VERSION"),
        "retry_interval_ms": policy.retry_delay().as_millis(),
        "transport_alpn": String::from_utf8_lossy(pab_transport::PAB_ALPN),
        "screenshot_schema": pab_protocol::SCREENSHOT_SCHEMA_VERSION,
    })
    .to_string()
}
