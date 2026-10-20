//! TCP destination policy shared by desktop Executor and Android.
use pab_protocol::PortForwardErrorCode;
use std::time::Duration;
use tokio::{net::TcpStream, time::Instant};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Debug)]
pub struct ConnectError {
    pub code: PortForwardErrorCode,
    pub message: String,
}

fn error(code: PortForwardErrorCode, message: impl Into<String>) -> ConnectError {
    ConnectError {
        code,
        message: message.into(),
    }
}

/// DNS and all sequential address attempts share one deadline. No retry takes
/// place after returning a connected socket or after application bytes flow.
pub async fn connect_target(host: &str, port: u16) -> Result<TcpStream, ConnectError> {
    if port == 0 || !pab_protocol::valid_port_forward_host(host) {
        return Err(error(
            PortForwardErrorCode::InvalidTarget,
            "invalid TCP target",
        ));
    }
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let addresses =
        match tokio::time::timeout_at(deadline, tokio::net::lookup_host((host, port))).await {
            Ok(Ok(addresses)) => addresses.collect::<Vec<_>>(),
            Ok(Err(cause)) => {
                return Err(error(
                    PortForwardErrorCode::DnsFailed,
                    format!("target DNS lookup failed: {cause}"),
                ));
            }
            Err(_) => {
                return Err(error(
                    PortForwardErrorCode::ConnectTimeout,
                    "target DNS lookup timed out",
                ));
            }
        };
    if addresses.is_empty() {
        return Err(error(
            PortForwardErrorCode::DnsFailed,
            "target DNS returned no addresses",
        ));
    }
    let mut last_error = None;
    for address in addresses {
        match tokio::time::timeout_at(deadline, TcpStream::connect(address)).await {
            Ok(Ok(connection)) => return Ok(connection),
            Ok(Err(cause)) => last_error = Some(cause),
            Err(_) => {
                return Err(error(
                    PortForwardErrorCode::ConnectTimeout,
                    "target TCP connect timed out",
                ));
            }
        }
    }
    let cause = last_error.expect("nonempty address list");
    let code = if cause.kind() == std::io::ErrorKind::ConnectionRefused {
        PortForwardErrorCode::ConnectionRefused
    } else {
        PortForwardErrorCode::ConnectFailed
    };
    Err(error(code, format!("target TCP connect failed: {cause}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn validates_before_dns_and_distinguishes_refused_target() {
        for (host, port) in [("https://example.com", 80), ("localhost", 0), ("a\nb", 80)] {
            assert_eq!(
                connect_target(host, port).await.unwrap_err().code,
                PortForwardErrorCode::InvalidTarget
            );
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let socket = connect_target("127.0.0.1", port).await.unwrap();
        let (accepted, _) = listener.accept().await.unwrap();
        drop((socket, accepted, listener));
        assert_eq!(
            connect_target("127.0.0.1", port).await.unwrap_err().code,
            PortForwardErrorCode::ConnectionRefused
        );
    }
}
