//! Per-worker local IPC. Peer identities come from the kernel, not message fields.
//! The parent retains the child's process handle until authentication completes.
//! No TCP listener, service credentials or inherited cross-session handles.
use super::UserIdentity;
use std::{io, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(unix)]
#[path = "channel_unix.rs"]
mod native;
#[cfg(windows)]
#[path = "channel_windows.rs"]
mod native;

pub trait WorkerIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> WorkerIo for T {}
pub type WorkerStream = Box<dyn WorkerIo>;

pub struct WorkerListener(native::Listener);
impl WorkerListener {
    /// Bind before spawning; only the target account can open this endpoint.
    pub fn bind(target: &UserIdentity) -> io::Result<Self> {
        native::Listener::bind(target).map(Self)
    }
    pub fn address(&self) -> &str {
        self.0.address()
    }
    /// Reject unrelated local clients without consuming the intended worker's
    /// connection. A single deadline bounds all rejected attempts combined.
    pub async fn accept(&mut self, child_pid: u32, timeout: Duration) -> io::Result<WorkerStream> {
        deadline(timeout, self.0.accept(child_pid)).await
    }
}

/// The worker verifies its server PID before reading any executable request.
pub async fn connect(
    address: &str,
    parent_pid: u32,
    timeout: Duration,
) -> io::Result<WorkerStream> {
    deadline(timeout, native::connect(address, parent_pid)).await
}

async fn deadline<T>(
    timeout: Duration,
    operation: impl std::future::Future<Output = io::Result<T>>,
) -> io::Result<T> {
    tokio::time::timeout(timeout, operation)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "worker IPC handshake timed out"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    const WAIT: Duration = Duration::from_secs(3);

    #[tokio::test]
    async fn channel_checks_kernel_peer_and_transfers_binary_without_tcp() {
        let mut listener =
            WorkerListener::bind(&super::super::current_identity().unwrap()).unwrap();
        let address = listener.address().to_owned();
        let client = tokio::spawn(async move {
            let mut stream = connect(&address, std::process::id(), WAIT).await.unwrap();
            stream.write_all(&[0, 255, 128, 1]).await.unwrap();
            let mut reply = [0; 4];
            stream.read_exact(&mut reply).await.unwrap();
            assert_eq!(reply, [1, 128, 255, 0]);
        });
        let mut stream = listener.accept(std::process::id(), WAIT).await.unwrap();
        let mut data = [0; 4];
        stream.read_exact(&mut data).await.unwrap();
        assert_eq!(data, [0, 255, 128, 1]);
        data.reverse();
        stream.write_all(&data).await.unwrap();
        client.await.unwrap();
    }

    #[tokio::test]
    async fn channel_rejects_wrong_server_pid_and_bounds_wrong_client_wait() {
        let mut listener =
            WorkerListener::bind(&super::super::current_identity().unwrap()).unwrap();
        let address = listener.address().to_owned();
        let client = tokio::spawn(async move {
            assert_eq!(
                connect(&address, u32::MAX, WAIT)
                    .await
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::PermissionDenied
            );
        });
        // Keep the accepted socket alive while the client checks the server.
        // Closing it concurrently can make Darwin report NotConnected first.
        let stream = listener.accept(std::process::id(), WAIT).await.unwrap();
        client.await.unwrap();
        drop(stream);

        let mut listener =
            WorkerListener::bind(&super::super::current_identity().unwrap()).unwrap();
        let address = listener.address().to_owned();
        let client = tokio::spawn(async move {
            let _ = connect(&address, std::process::id(), WAIT).await;
        });
        assert_eq!(
            listener
                .accept(u32::MAX, Duration::from_millis(150))
                .await
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::TimedOut
        );
        client.await.unwrap();
    }
}
