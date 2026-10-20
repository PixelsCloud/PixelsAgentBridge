//! Connection-scoped TCP relays. No tunnel or payload is persisted or replayed.
use crate::execution::ExecutionService;
use pab_device_session::port_forward::{IDLE_TIMEOUT, connect_target};
use pab_protocol::{DeviceTaskResponse, PORT_FORWARD_SCHEMA_VERSION, PortForwardErrorCode};
use pab_transport::{PabBiStream, PabConnectionError};
use std::{
    sync::{Arc, atomic::AtomicU64},
    time::Duration,
};
use tokio::sync::Semaphore;

#[cfg(test)]
#[path = "port_forward_tests.rs"]
mod tests;

pub(crate) struct PortForwardService {
    gate: Arc<ExecutionService>,
    slots: Arc<Semaphore>,
}

impl PortForwardService {
    pub fn new(gate: Arc<ExecutionService>) -> Arc<Self> {
        Arc::new(Self {
            gate,
            slots: Arc::new(Semaphore::new(8)),
        })
    }

    pub async fn serve(
        &self,
        mut stream: PabBiStream,
        session_slots: Arc<Semaphore>,
        version: u16,
        host: &str,
        port: u16,
        timeout: Duration,
    ) -> Result<(), PabConnectionError> {
        if version != PORT_FORWARD_SCHEMA_VERSION {
            return reject(
                &mut stream,
                timeout,
                PortForwardErrorCode::Unsupported,
                "unsupported port forward protocol",
            )
            .await;
        }
        if port == 0 || !pab_protocol::valid_port_forward_host(host) {
            return reject(
                &mut stream,
                timeout,
                PortForwardErrorCode::InvalidTarget,
                "invalid TCP target",
            )
            .await;
        }
        let Ok(_global) = self.slots.clone().try_acquire_owned() else {
            return reject(
                &mut stream,
                timeout,
                PortForwardErrorCode::ResourceLimit,
                "Android TCP relay limit reached",
            )
            .await;
        };
        let Ok(_session) = session_slots.try_acquire_owned() else {
            return reject(
                &mut stream,
                timeout,
                PortForwardErrorCode::ResourceLimit,
                "device session TCP relay limit reached",
            )
            .await;
        };
        if let Err(error) = self.gate.authorize().await {
            return stream
                .send_json(
                    &DeviceTaskResponse::Error {
                        code: error.0,
                        message: error.1.into(),
                    },
                    timeout,
                )
                .await;
        }
        // This monitor spans dialing, the framed handshake and raw transport.
        // Dropping work drops the destination socket and both QUIC directions.
        let monitor = async {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                if self.gate.authorize().await.is_err() {
                    return;
                }
            }
        };
        let work = async {
            let tcp = match connect_target(host, port).await {
                Ok(tcp) => tcp,
                Err(error) => {
                    return reject(&mut stream, timeout, error.code, &error.message).await;
                }
            };
            // Do not acknowledge a new relay if control changed while dialing.
            if let Err(error) = self.gate.authorize().await {
                return stream
                    .send_json(
                        &DeviceTaskResponse::Error {
                            code: error.0,
                            message: error.1.into(),
                        },
                        timeout,
                    )
                    .await;
            }
            stream
                .send_frame_json(&DeviceTaskResponse::PortForwardConnected, timeout)
                .await?;
            stream
                .relay_tcp(tcp, &AtomicU64::new(0), &AtomicU64::new(0), IDLE_TIMEOUT)
                .await
        };
        tokio::select! {
            result = work => result,
            _ = monitor => Ok(()),
        }
    }
}

async fn reject(
    stream: &mut PabBiStream,
    timeout: Duration,
    code: PortForwardErrorCode,
    message: &str,
) -> Result<(), PabConnectionError> {
    stream
        .send_json(
            &DeviceTaskResponse::PortForwardRejected {
                code,
                message: message.into(),
            },
            timeout,
        )
        .await
}
