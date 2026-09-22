use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream};
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use zeroize::Zeroizing;

pub const MAX_PAB_MESSAGE_BYTES: usize = 64 * 1024;

pub struct PabConnection {
    inner: Connection,
}

impl PabConnection {
    pub(crate) fn new(inner: Connection) -> Self {
        Self { inner }
    }

    pub fn remote_endpoint_key(&self) -> [u8; 32] {
        *self.inner.remote_id().as_bytes()
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
    pub async fn send_json<T: Serialize>(
        &mut self,
        value: &T,
        timeout: Duration,
    ) -> Result<(), PabConnectionError> {
        let encoded = serde_json::to_vec(value)?;
        self.send_encoded(&encoded, timeout).await
    }

    pub async fn send_sensitive_json<T: Serialize>(
        &mut self,
        value: &T,
        timeout: Duration,
    ) -> Result<(), PabConnectionError> {
        let encoded = Zeroizing::new(serde_json::to_vec(value)?);
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
            let mut encoded = vec![0_u8; length];
            self.receive
                .read_exact(&mut encoded)
                .await
                .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
            Ok(serde_json::from_slice(&encoded)?)
        })
        .await
        .map_err(|_| PabConnectionError::Timeout)?
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
    #[error("PAB stream failed: {0}")]
    Stream(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
