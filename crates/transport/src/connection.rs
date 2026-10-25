use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use iroh::endpoint::{Connection, PathEvent, RecvStream, SendStream};
use n0_future::StreamExt;
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::watch,
    time::Instant,
};
use zeroize::Zeroizing;

pub const MAX_PAB_MESSAGE_BYTES: usize = 64 * 1024;
pub const MAX_BINARY_FRAME_BYTES: usize = 256 * 1024;

pub struct PabFrameWriter(SendStream);
pub struct PabFrameReader(RecvStream);
impl PabFrameWriter {
    pub async fn write(&mut self, bytes: &[u8], limit: usize) -> Result<(), PabConnectionError> {
        if bytes.is_empty() || bytes.len() > limit.min(MAX_BINARY_FRAME_BYTES) {
            return Err(PabConnectionError::InvalidBinaryFrame(bytes.len()));
        }
        self.0
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .await
            .map_err(|e| PabConnectionError::Stream(e.to_string()))?;
        self.0
            .write_all(bytes)
            .await
            .map_err(|e| PabConnectionError::Stream(e.to_string()))
    }
    pub fn finish(&mut self) -> Result<(), PabConnectionError> {
        self.0
            .finish()
            .map_err(|e| PabConnectionError::Stream(e.to_string()))
    }
}
impl PabFrameReader {
    /// Wait for an idle producer without a deadline, then bound the partial
    /// length/header read. Callers must drop this reader on error/cancellation.
    /// Existing task/terminal reads retain their original timeout behavior.
    pub async fn read_after_idle(
        &mut self,
        limit: usize,
        timeout: Duration,
    ) -> Result<Vec<u8>, PabConnectionError> {
        let mut header = [0; 4];
        self.0
            .read_exact(&mut header[..1])
            .await
            .map_err(|e| PabConnectionError::Stream(e.to_string()))?;
        tokio::time::timeout(timeout, async {
            self.0
                .read_exact(&mut header[1..])
                .await
                .map_err(|e| PabConnectionError::Stream(e.to_string()))?;
            let len = u32::from_be_bytes(header) as usize;
            if len == 0 || len > limit.min(MAX_BINARY_FRAME_BYTES) {
                return Err(PabConnectionError::InvalidBinaryFrame(len));
            }
            let mut bytes = vec![0; len];
            self.0
                .read_exact(&mut bytes)
                .await
                .map_err(|e| PabConnectionError::Stream(e.to_string()))?;
            Ok(bytes)
        })
        .await
        .map_err(|_| PabConnectionError::Timeout)?
    }
    pub async fn read(&mut self, limit: usize) -> Result<Vec<u8>, PabConnectionError> {
        let mut header = [0; 4];
        self.0
            .read_exact(&mut header)
            .await
            .map_err(|e| PabConnectionError::Stream(e.to_string()))?;
        let len = u32::from_be_bytes(header) as usize;
        if len == 0 || len > limit.min(MAX_BINARY_FRAME_BYTES) {
            return Err(PabConnectionError::InvalidBinaryFrame(len));
        }
        let mut bytes = vec![0; len];
        self.0
            .read_exact(&mut bytes)
            .await
            .map_err(|e| PabConnectionError::Stream(e.to_string()))?;
        Ok(bytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionPath {
    Direct,
    Relay,
    Unknown,
}

#[derive(Clone)]
pub struct PabConnection {
    inner: Connection,
    traffic: std::sync::Arc<std::sync::Mutex<crate::traffic::PathTraffic>>,
}

impl PabConnection {
    pub(crate) fn new(inner: Connection) -> Self {
        let traffic =
            std::sync::Arc::new(std::sync::Mutex::new(crate::traffic::PathTraffic::default()));
        let events_traffic = traffic.clone();
        // Subscribe before inspecting paths, so closing paths retain their final counters.
        let mut events = inner.path_events();
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
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                match &event {
                    PathEvent::Closed {
                        id,
                        remote_addr,
                        last_stats,
                        ..
                    } if remote_addr.is_ip() => {
                        if let Ok(mut counters) = events_traffic.lock() {
                            counters.observe(
                                id.to_string(),
                                last_stats.udp_tx.bytes,
                                last_stats.udp_rx.bytes,
                            );
                        }
                    }
                    PathEvent::Lagged { .. } => {
                        if let Ok(mut counters) = events_traffic.lock() {
                            counters.mark_incomplete();
                        }
                    }
                    _ => {}
                }
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
        Self { inner, traffic }
    }

    /// UDP payload bytes on direct QUIC paths, including QUIC overhead and
    /// retransmissions. Only the controlling endpoint reports these counters.
    pub fn direct_traffic(&self) -> crate::DirectTraffic {
        let Ok(mut counters) = self.traffic.lock() else {
            return crate::DirectTraffic {
                incomplete: true,
                ..Default::default()
            };
        };
        for path in self.inner.paths().iter().filter(|path| path.is_ip()) {
            let stats = path.stats();
            counters.observe(
                path.id().to_string(),
                stats.udp_tx.bytes,
                stats.udp_rx.bytes,
            );
        }
        counters.total()
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
    /// QUIC's estimate for the currently selected path. This is round trip,
    /// independent of video PTS and includes Relay transit when relayed.
    pub fn selected_path_rtt(&self) -> Option<Duration> {
        if self.inner.close_reason().is_some() { return None; }
        self.inner.paths().iter().find(|path| path.is_selected())
            .and_then(|path| self.inner.rtt(path.id()))
    }
    pub fn statistics(&self) -> Option<crate::ConnectionStatistics> {
        crate::statistics::snapshot(&self.inner)
    }

    pub async fn open_bi(&self, timeout: Duration) -> Result<PabBiStream, PabConnectionError> {
        let (send, receive) = tokio::time::timeout(timeout, self.inner.open_bi())
            .await
            .map_err(|_| PabConnectionError::Timeout)?
            .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
        Ok(PabBiStream { send, receive, connection: self.inner.clone() })
    }

    pub async fn accept_bi(&self, timeout: Duration) -> Result<PabBiStream, PabConnectionError> {
        let (send, receive) = tokio::time::timeout(timeout, self.inner.accept_bi())
            .await
            .map_err(|_| PabConnectionError::Timeout)?
            .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
        Ok(PabBiStream { send, receive, connection: self.inner.clone() })
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
    connection: Connection,
}

impl PabBiStream {
    pub fn statistics(&self) -> Option<crate::ConnectionStatistics> {
        crate::statistics::snapshot(&self.connection)
    }
    /// Independent owned directions for additive streaming protocols. Readers
    /// must stay alive across idle periods: never cancel a partial frame read.
    pub fn into_framed(self) -> (PabFrameWriter, PabFrameReader) {
        (PabFrameWriter(self.send), PabFrameReader(self.receive))
    }
    /// After the framed request and reply, forward unframed bytes in both
    /// directions. FIN is propagated independently; a reset or idle timeout
    /// drops both halves. Counters include only bytes written to the peer.
    pub async fn relay_tcp(
        self,
        tcp: TcpStream,
        to_remote: &AtomicU64,
        to_local: &AtomicU64,
        idle_timeout: Duration,
    ) -> Result<(), PabConnectionError> {
        let (mut tcp_read, mut tcp_write) = tcp.into_split();
        let mut quic_send = self.send;
        let mut quic_receive = self.receive;
        let (activity, mut last_activity) = watch::channel(Instant::now());
        let upload_activity = activity.clone();
        let upload = async move {
            let mut buffer = [0_u8; 16 * 1024];
            loop {
                let count = tcp_read
                    .read(&mut buffer)
                    .await
                    .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
                if count == 0 {
                    quic_send
                        .finish()
                        .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
                    return Ok::<(), PabConnectionError>(());
                }
                quic_send
                    .write_all(&buffer[..count])
                    .await
                    .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
                to_remote.fetch_add(count as u64, Ordering::Relaxed);
                upload_activity.send_replace(Instant::now());
            }
        };
        let download = async move {
            let mut buffer = [0_u8; 16 * 1024];
            loop {
                let count = quic_receive
                    .read(&mut buffer)
                    .await
                    .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
                let Some(count) = count else {
                    tcp_write
                        .shutdown()
                        .await
                        .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
                    return Ok::<(), PabConnectionError>(());
                };
                tcp_write
                    .write_all(&buffer[..count])
                    .await
                    .map_err(|error| PabConnectionError::Stream(error.to_string()))?;
                to_local.fetch_add(count as u64, Ordering::Relaxed);
                activity.send_replace(Instant::now());
            }
        };
        let pump = async { tokio::try_join!(upload, download).map(|_| ()) };
        let idle = async {
            loop {
                let deadline = *last_activity.borrow() + idle_timeout;
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => {
                        if Instant::now() >= *last_activity.borrow() + idle_timeout {
                            return PabConnectionError::Timeout;
                        }
                    }
                    changed = last_activity.changed() => {
                        if changed.is_err() {
                            return PabConnectionError::Stream("relay activity ended".into());
                        }
                    }
                }
            }
        };
        tokio::select! {
            result = pump => result,
            error = idle => Err(error),
        }
    }

    /// Require the peer's FIN after an exact-size binary payload. An extra byte
    /// is a protocol error, including an incomplete trailing frame header.
    pub async fn expect_receive_end(
        &mut self,
        timeout: Duration,
    ) -> Result<(), PabConnectionError> {
        let mut byte = [0_u8; 1];
        match tokio::time::timeout(timeout, self.receive.read(&mut byte))
            .await
            .map_err(|_| PabConnectionError::Timeout)?
            .map_err(|error| PabConnectionError::Stream(error.to_string()))?
        {
            None => Ok(()),
            Some(_) => Err(PabConnectionError::Stream(
                "unexpected bytes after payload".to_owned(),
            )),
        }
    }
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

#[cfg(test)]
mod traffic_tests {
    use super::*;

    #[tokio::test]
    async fn direct_quic_transfer_counts_real_bytes_in_both_directions() {
        let bind = |key| {
            iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
                .secret_key(iroh::SecretKey::from_bytes(&[key; 32]))
                .alpns(vec![crate::PAB_ALPN.to_vec()])
                .relay_mode(iroh::RelayMode::Disabled)
                .bind()
        };
        let a = bind(41).await.unwrap();
        let b = bind(42).await.unwrap();
        let peer = b.addr();
        let (outgoing, incoming) = tokio::join!(a.connect(peer, crate::PAB_ALPN), async {
            b.accept().await.unwrap().await.unwrap()
        });
        let a_connection = PabConnection::new(outgoing.unwrap());
        let b_connection = PabConnection::new(incoming);
        let payload = vec![17_u8; 32 * 1024];
        let before = a_connection.direct_traffic();
        let sender = async {
            let mut stream = a_connection.open_bi(Duration::from_secs(10)).await.unwrap();
            stream
                .send_binary_frame(&payload, Duration::from_secs(10))
                .await
                .unwrap();
            let reply = stream
                .receive_binary_frame(Duration::from_secs(10))
                .await
                .unwrap();
            assert_eq!(reply, payload);
        };
        let receiver = async {
            let mut stream = b_connection
                .accept_bi(Duration::from_secs(10))
                .await
                .unwrap();
            let received = stream
                .receive_binary_frame(Duration::from_secs(10))
                .await
                .unwrap();
            stream
                .send_binary_frame(&received, Duration::from_secs(10))
                .await
                .unwrap();
        };
        tokio::time::timeout(Duration::from_secs(20), async {
            tokio::join!(sender, receiver);
        })
        .await
        .unwrap();
        let after = a_connection.direct_traffic();
        assert!(after.uploaded_bytes > before.uploaded_bytes + 32 * 1024);
        assert!(after.downloaded_bytes > before.downloaded_bytes + 32 * 1024);
        assert!(!after.incomplete);
        a_connection.close(b"done");
        a.close().await;
        b.close().await;
    }
}
