use std::time::Duration;

use iroh::endpoint::{Connection, PathEvent, RecvStream, SendStream};
use n0_future::StreamExt;
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use zeroize::Zeroizing;

pub const MAX_PAB_MESSAGE_BYTES: usize = 64 * 1024;
pub const MAX_BINARY_FRAME_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionPath {
    Direct,
    Relay,
    Unknown,
}

#[derive(Clone)]
pub struct PabConnection {
    inner: Connection,
}

impl PabConnection {
    pub(crate) fn new(inner: Connection) -> Self {
        let initial = inner.paths();
        if let Some(path) = initial.iter().find(|path| path.is_selected()) {
            let selected = if path.is_ip() {
                "direct"
            } else if path.is_relay() {
                "relay"
            } else {
                "unknown"
            };
            tracing::info!(selected_path = selected, "PAB connection path selected");
        }
        let mut events = inner.path_events();
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                if let PathEvent::Selected { remote_addr, .. } = event {
                    let selected = if remote_addr.is_ip() {
                        "direct"
                    } else if remote_addr.is_relay() {
                        "relay"
                    } else {
                        "unknown"
                    };
                    tracing::info!(selected_path = selected, "PAB connection path selected");
                }
            }
        });
        Self { inner }
    }

    pub fn remote_endpoint_key(&self) -> [u8; 32] {
        *self.inner.remote_id().as_bytes()
    }

    pub fn selected_path(&self) -> Option<ConnectionPath> {
        if self.inner.close_reason().is_some() {
            return None;
        }

        Some(
            self.inner
                .paths()
                .iter()
                .find(|path| path.is_selected())
                .map(|path| {
                    if path.is_ip() {
                        ConnectionPath::Direct
                    } else if path.is_relay() {
                        ConnectionPath::Relay
                    } else {
                        ConnectionPath::Unknown
                    }
                })
                .unwrap_or(ConnectionPath::Unknown),
        )
    }

    pub async fn open_bi(&self, timeout: Duration) -> Result<PabBiStream, PabConnectionError> {
        let (send, receive) = tokio::time::timeout(timeout, self.inner.open_bi())
            .await
            .map_err(|_| PabConnectionError::Timeout)?
            .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
        Ok(PabBiStream { send, receive })
    }

    pub async fn accept_bi(&self, timeout: Duration) -> Result<PabBiStream, PabConnectionError> {
        let (send, receive) = tokio::time::timeout(timeout, self.inner.accept_bi())
            .await
            .map_err(|_| PabConnectionError::Timeout)?
            .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
        Ok(PabBiStream { send, receive })
    }

    pub async fn closed(&self) {
        let _ = self.inner.closed().await;
    }

    pub fn close(&self, reason: &[u8]) {
        self.inner.close(0_u8.into(), reason);
    }
}

pub struct PabBiStream {
    send: SendStream,
    receive: RecvStream,
}

impl PabBiStream {
    pub async fn send_binary_frame(
        &mut self,
        bytes: &[u8],
        timeout: Duration,
    ) -> Result<(), PabConnectionError> {
        if bytes.is_empty() || bytes.len() > MAX_BINARY_FRAME_BYTES {
            return Err(PabConnectionError::InvalidBinaryFrame(bytes.len()));
        }
        let length = u32::try_from(bytes.len())
            .map_err(|_| PabConnectionError::InvalidBinaryFrame(bytes.len()))?
            .to_be_bytes();
        tokio::time::timeout(timeout, async {
            self.send
                .write_all(&length)
                .await
                .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
            self.send
                .write_all(bytes)
                .await
                .map_err(|error| PabConnectionError::Stream(error.to_string()))
        })
        .await
        .map_err(|_| PabConnectionError::Timeout)?
    }

    pub async fn receive_binary_frame(
        &mut self,
        timeout: Duration,
    ) -> Result<Vec<u8>, PabConnectionError> {
        tokio::time::timeout(timeout, async {
            let mut length = [0_u8; 4];
            self.receive
                .read_exact(&mut length)
                .await
                .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
            let length = u32::from_be_bytes(length) as usize;
            if length == 0 || length > MAX_BINARY_FRAME_BYTES {
                return Err(PabConnectionError::InvalidBinaryFrame(length));
            }
            let mut bytes = vec![0_u8; length];
            let mut offset = 0;
            while offset < length {
                let end = (offset + 16 * 1024).min(length);
                self.receive
                    .read_exact(&mut bytes[offset..end])
                    .await
                    .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
                offset = end;
            }
            Ok(bytes)
        })
        .await
        .map_err(|_| PabConnectionError::Timeout)?
    }

    pub async fn send_json<T: Serialize>(
        &mut self,
        value: &T,
        timeout: Duration,
    ) -> Result<(), PabConnectionError> {
        self.send_frame_json(value, timeout).await?;
        self.finish_send(timeout).await
    }

    pub async fn send_sensitive_json<T: Serialize>(
        &mut self,
        value: &T,
        timeout: Duration,
    ) -> Result<(), PabConnectionError> {
        let encoded = Zeroizing::new(serde_json::to_vec(value)?);
        self.send_encoded(&encoded, timeout).await?;
        self.finish_send(timeout).await
    }

    pub async fn send_frame_json<T: Serialize>(
        &mut self,
        value: &T,
        timeout: Duration,
    ) -> Result<(), PabConnectionError> {
        let encoded = serde_json::to_vec(value)?;
        self.send_encoded(&encoded, timeout).await
    }

    async fn send_encoded(
        &mut self,
        encoded: &[u8],
        timeout: Duration,
    ) -> Result<(), PabConnectionError> {
        if encoded.len() > MAX_PAB_MESSAGE_BYTES {
            return Err(PabConnectionError::MessageTooLarge(encoded.len()));
        }
        let length = u32::try_from(encoded.len())
            .map_err(|_| PabConnectionError::MessageTooLarge(encoded.len()))?
            .to_be_bytes();
        tokio::time::timeout(timeout, async {
            self.send
                .write_all(&length)
                .await
                .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
            self.send
                .write_all(encoded)
                .await
                .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|_| PabConnectionError::Timeout)?
    }

    pub async fn finish_send(&mut self, timeout: Duration) -> Result<(), PabConnectionError> {
        tokio::time::timeout(timeout, async {
            self.send
                .finish()
                .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
            match self
                .send
                .stopped()
                .await
                .map_err(|error| PabConnectionError::Stream(error.to_string()))?
            {
                None => Ok(()),
                Some(code) if code == 0_u8.into() => Ok(()),
                Some(code) => Err(PabConnectionError::Stream(format!(
                    "peer stopped the message stream with code {code}"
                ))),
            }
        })
        .await
        .map_err(|_| PabConnectionError::Timeout)?
    }

    pub async fn receive_json<T: DeserializeOwned>(
        &mut self,
        timeout: Duration,
    ) -> Result<T, PabConnectionError> {
        tokio::time::timeout(timeout, self.receive_json_wait())
            .await
            .map_err(|_| PabConnectionError::Timeout)?
    }

    pub async fn receive_json_wait<T: DeserializeOwned>(
        &mut self,
    ) -> Result<T, PabConnectionError> {
        let mut length = [0_u8; 4];
        self.receive
            .read_exact(&mut length)
            .await
            .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
        let length = usize::try_from(u32::from_be_bytes(length))
            .map_err(|_| PabConnectionError::MessageTooLarge(usize::MAX))?;
        if length > MAX_PAB_MESSAGE_BYTES {
            return Err(PabConnectionError::MessageTooLarge(length));
        }
        let mut encoded = vec![0_u8; length];
        self.receive
            .read_exact(&mut encoded)
            .await
            .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
        Ok(serde_json::from_slice(&encoded)?)
    }

    pub async fn receive_sensitive_json<T: DeserializeOwned>(
        &mut self,
        timeout: Duration,
    ) -> Result<T, PabConnectionError> {
        tokio::time::timeout(timeout, async {
            let mut length = [0_u8; 4];
            self.receive
                .read_exact(&mut length)
                .await
                .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
            let length = usize::try_from(u32::from_be_bytes(length))
                .map_err(|_| PabConnectionError::MessageTooLarge(usize::MAX))?;
            if length > MAX_PAB_MESSAGE_BYTES {
                return Err(PabConnectionError::MessageTooLarge(length));
            }
            let mut encoded = Zeroizing::new(vec![0_u8; length]);
            self.receive
                .read_exact(&mut encoded)
                .await
                .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
            Ok(serde_json::from_slice(&encoded)?)
        })
        .await
        .map_err(|_| PabConnectionError::Timeout)?
    }
}

#[derive(Debug, Error)]
pub enum PabConnectionError {
    #[error("PAB connection operation timed out")]
    Timeout,
    #[error("PAB message is too large: {0} bytes")]
    MessageTooLarge(usize),
    #[error("invalid binary frame length: {0} bytes")]
    InvalidBinaryFrame(usize),
    #[error("PAB stream failed: {0}")]
    Stream(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
