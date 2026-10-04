use std::{net::SocketAddr, path::PathBuf, time::Duration};

use super::config::{RelayServiceConfig, RelayServiceConfigError};

fn config() -> RelayServiceConfig {
    RelayServiceConfig {
        control_url: "wss://localhost/control".to_owned(),
        control_secret: "a test Relay control secret with 32 bytes".to_owned(),
        control_ca_cert: None,
        tls_cert: PathBuf::from("cert.pem"),
        tls_key: PathBuf::from("key.pem"),
        https_bind: "127.0.0.1:31443".parse().unwrap(),
        captive_bind: "127.0.0.1:0".parse().unwrap(),
        quic_bind: "0.0.0.0:7842".parse().unwrap(),
        policy_refresh_interval: Duration::from_secs(20),
        reconnect_interval: Duration::from_secs(3),
        limiter_burst: Duration::from_millis(100),
    }
}

#[test]
fn requires_tls_for_the_pab_control_connection() {
    let mut config = config();
    config.control_url = "ws://localhost/control".to_owned();
    assert_eq!(
        config.validate(),
        Err(RelayServiceConfigError::ControlUrlMustUseTls)
    );
}

#[test]
fn keeps_the_upstream_captive_portal_on_loopback() {
    let mut config = config();
    config.captive_bind = SocketAddr::from(([0, 0, 0, 0], 8080));
    assert_eq!(
        config.validate(),
        Err(RelayServiceConfigError::CaptivePortalMustBeLoopback)
    );
}
