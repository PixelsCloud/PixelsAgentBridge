use std::{sync::atomic::AtomicU64, time::Duration};

use pab_protocol::{
    DeviceTaskResponse, PORT_FORWARD_SCHEMA_VERSION, PortForwardErrorCode, valid_port_forward_host,
};
use pab_transport::PabBiStream;
use tokio::{net::TcpStream, time::Instant};

use super::{TaskService, TaskServiceError};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

impl TaskService {
    pub(super) async fn port_forward_stream(
        &self,
        mut stream: PabBiStream,
        version: u16,
        host: &str,
        port: u16,
        timeout: Duration,
    ) -> Result<(), TaskServiceError> {
        if version != PORT_FORWARD_SCHEMA_VERSION {
            return reject(
                &mut stream,
                timeout,
                PortForwardErrorCode::Unsupported,
                "unsupported port forward protocol",
            )
            .await;
        }
        if port == 0 || !valid_port_forward_host(host) {
            return reject(
                &mut stream,
                timeout,
                PortForwardErrorCode::InvalidTarget,
                "invalid TCP target",
            )
            .await;
        }
        let Ok(_global_slot) = self.port_forward_global_slots.clone().try_acquire_owned() else {
            return reject(
                &mut stream,
                timeout,
                PortForwardErrorCode::ResourceLimit,
                "Executor TCP relay limit reached",
            )
            .await;
        };
        let Ok(_session_slot) = self.port_forward_session_slots.clone().try_acquire_owned() else {
            return reject(
                &mut stream,
                timeout,
                PortForwardErrorCode::ResourceLimit,
                "device session TCP relay limit reached",
            )
            .await;
        };
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let addresses =
            match tokio::time::timeout_at(deadline, tokio::net::lookup_host((host, port))).await {
                Ok(Ok(addresses)) => addresses.collect::<Vec<_>>(),
                Ok(Err(error)) => {
                    return reject(
                        &mut stream,
                        timeout,
                        PortForwardErrorCode::DnsFailed,
                        &format!("target DNS lookup failed: {error}"),
                    )
                    .await;
                }
                Err(_) => {
                    return reject(
                        &mut stream,
                        timeout,
                        PortForwardErrorCode::ConnectTimeout,
                        "target DNS lookup timed out",
                    )
                    .await;
                }
            };
        if addresses.is_empty() {
            return reject(
                &mut stream,
                timeout,
                PortForwardErrorCode::DnsFailed,
                "target DNS returned no addresses",
            )
            .await;
        }
        let mut last_error = None;
        let mut tcp = None;
        for address in addresses {
            match tokio::time::timeout_at(deadline, TcpStream::connect(address)).await {
                Ok(Ok(connection)) => {
                    tcp = Some(connection);
                    break;
                }
                Ok(Err(error)) => last_error = Some(error),
                Err(_) => {
                    return reject(
                        &mut stream,
                        timeout,
                        PortForwardErrorCode::ConnectTimeout,
                        "target TCP connect timed out",
                    )
                    .await;
                }
            }
        }
        let Some(tcp) = tcp else {
            let error = last_error.expect("nonempty address list");
            let code = if error.kind() == std::io::ErrorKind::ConnectionRefused {
                PortForwardErrorCode::ConnectionRefused
            } else {
                PortForwardErrorCode::ConnectFailed
            };
            return reject(
                &mut stream,
                timeout,
                code,
                &format!("target TCP connect failed: {error}"),
            )
            .await;
        };
        stream
            .send_frame_json(&DeviceTaskResponse::PortForwardConnected, timeout)
            .await?;
        // The stream owns both QUIC halves after the handshake. Move it out
        // only after writing the success response, preserving the raw phase.
        let sent = AtomicU64::new(0);
        let received = AtomicU64::new(0);
        stream
            .relay_tcp(tcp, &sent, &received, IDLE_TIMEOUT)
            .await?;
        Ok(())
    }
}

async fn reject(
    stream: &mut PabBiStream,
    timeout: Duration,
    code: PortForwardErrorCode,
    message: &str,
) -> Result<(), TaskServiceError> {
    stream
        .send_json(
            &DeviceTaskResponse::PortForwardRejected {
                code,
                message: message.to_owned(),
            },
            timeout,
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_service::transfer_tests::{actor, pair, service};
    use pab_protocol::{DEVICE_TASK_SCHEMA_VERSION, DeviceTaskRequest};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn raw_tcp_relay_preserves_binary_and_independent_half_close() {
        let directory = tempfile::tempdir().unwrap();
        let svc = service(directory.path()).await.for_ui_connection();
        let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_port = target.local_addr().unwrap().port();
        let target_task = tokio::spawn(async move {
            let (mut socket, _) = target.accept().await.unwrap();
            let mut request = Vec::new();
            socket.read_to_end(&mut request).await.unwrap();
            socket.write_all(&request).await.unwrap();
            socket.shutdown().await.unwrap();
            request
        });
        let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = local.local_addr().unwrap();
        let (first, second, client, server) = pair().await;
        let server_for_task = server.clone();
        let remote_task = tokio::spawn(async move {
            svc.handle_stream(
                actor(),
                server_for_task
                    .accept_bi(Duration::from_secs(5))
                    .await
                    .unwrap(),
                Duration::from_secs(5),
            )
            .await
        });
        let local_task = tokio::spawn(async move {
            let (tcp, _) = local.accept().await.unwrap();
            let mut stream = client.open_bi(Duration::from_secs(5)).await.unwrap();
            stream
                .send_frame_json(
                    &DeviceTaskRequest::PortForwardConnect {
                        schema_version: DEVICE_TASK_SCHEMA_VERSION,
                        port_forward_schema_version: PORT_FORWARD_SCHEMA_VERSION,
                        host: "127.0.0.1".into(),
                        port: target_port,
                    },
                    Duration::from_secs(5),
                )
                .await
                .unwrap();
            assert_eq!(
                stream
                    .receive_json::<DeviceTaskResponse>(Duration::from_secs(5))
                    .await
                    .unwrap(),
                DeviceTaskResponse::PortForwardConnected
            );
            stream
                .relay_tcp(
                    tcp,
                    &AtomicU64::new(0),
                    &AtomicU64::new(0),
                    Duration::from_secs(5),
                )
                .await
        });
        let mut app = TcpStream::connect(local_addr).await.unwrap();
        let payload = (0..150_000_u32)
            .map(|n| (n.wrapping_mul(37) % 256) as u8)
            .collect::<Vec<_>>();
        app.write_all(&payload).await.unwrap();
        app.shutdown().await.unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), app.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        let target_received = tokio::time::timeout(Duration::from_secs(10), target_task)
            .await
            .unwrap()
            .unwrap();
        let local_result = local_task.await.unwrap();
        let remote_result = remote_task.await.unwrap();
        assert_eq!(
            response.len(),
            payload.len(),
            "local={local_result:?} remote={remote_result:?} target_bytes={}",
            target_received.len()
        );
        assert!(response == payload, "binary response differs");
        assert!(target_received == payload, "target request differs");
        local_result.unwrap();
        remote_result.unwrap();
        drop((first, second, server));
    }

    #[tokio::test]
    async fn refused_target_rejects_before_raw_phase() {
        let directory = tempfile::tempdir().unwrap();
        let svc = service(directory.path()).await.for_ui_connection();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unused_port = listener.local_addr().unwrap().port();
        drop(listener);
        let (first, second, client, server) = pair().await;
        let remote_task = tokio::spawn(async move {
            svc.handle_stream(
                actor(),
                server.accept_bi(Duration::from_secs(5)).await.unwrap(),
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        });
        let mut stream = client.open_bi(Duration::from_secs(5)).await.unwrap();
        stream
            .send_frame_json(
                &DeviceTaskRequest::PortForwardConnect {
                    schema_version: DEVICE_TASK_SCHEMA_VERSION,
                    port_forward_schema_version: PORT_FORWARD_SCHEMA_VERSION,
                    host: "127.0.0.1".into(),
                    port: unused_port,
                },
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        assert!(matches!(
            stream
                .receive_json::<DeviceTaskResponse>(Duration::from_secs(5))
                .await
                .unwrap(),
            DeviceTaskResponse::PortForwardRejected {
                code: PortForwardErrorCode::ConnectionRefused,
                ..
            }
        ));
        remote_task.await.unwrap();
        drop((first, second));
    }

    #[tokio::test]
    async fn exhausted_session_limit_rejects_only_the_new_stream() {
        let directory = tempfile::tempdir().unwrap();
        let mut svc = service(directory.path()).await.for_ui_connection();
        svc.port_forward_session_slots = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
        let (first, second, client, server) = pair().await;
        let remote_task = tokio::spawn(async move {
            svc.handle_stream(
                actor(),
                server.accept_bi(Duration::from_secs(5)).await.unwrap(),
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        });
        let mut stream = client.open_bi(Duration::from_secs(5)).await.unwrap();
        stream
            .send_frame_json(
                &DeviceTaskRequest::PortForwardConnect {
                    schema_version: DEVICE_TASK_SCHEMA_VERSION,
                    port_forward_schema_version: PORT_FORWARD_SCHEMA_VERSION,
                    host: "127.0.0.1".into(),
                    port: 12345,
                },
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        assert!(matches!(
            stream
                .receive_json::<DeviceTaskResponse>(Duration::from_secs(5))
                .await
                .unwrap(),
            DeviceTaskResponse::PortForwardRejected {
                code: PortForwardErrorCode::ResourceLimit,
                ..
            }
        ));
        remote_task.await.unwrap();
        assert!(
            client.selected_path().is_some(),
            "resource limit must not close the device connection"
        );
        drop((first, second));
    }

    #[test]
    fn host_validation_rejects_urls_and_accepts_ip_literals() {
        assert!(!valid_port_forward_host("https://example.com"));
        assert!(!valid_port_forward_host("example.com:443"));
        assert!(!valid_port_forward_host("host\nname"));
        assert!(valid_port_forward_host("::1"));
        assert!(valid_port_forward_host("example.internal"));
    }

    #[tokio::test]
    async fn idle_relay_closes_without_waiting_for_the_production_interval() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let app = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (tcp, _) = listener.accept().await.unwrap();
        let (first, second, client, server) = pair().await;
        let stream = client.open_bi(Duration::from_secs(5)).await.unwrap();
        let sent = AtomicU64::new(0);
        let received = AtomicU64::new(0);
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            stream.relay_tcp(tcp, &sent, &received, Duration::from_millis(100)),
        )
        .await
        .unwrap();
        assert!(matches!(
            result,
            Err(pab_transport::PabConnectionError::Timeout)
        ));
        assert_eq!(sent.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(received.load(std::sync::atomic::Ordering::Relaxed), 0);
        drop((app, first, second, server));
    }
}
