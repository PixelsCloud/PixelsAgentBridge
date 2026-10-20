//! Drain native diagnostics independently of media. Keep only a bounded tail,
//! including the final exception that can arrive just before stdout closes.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt};

const LIMIT: usize = 2048;
pub(super) struct Diagnostics {
    tail: Arc<Mutex<VecDeque<u8>>>,
    reader: tokio::task::JoinHandle<()>,
}
impl Diagnostics {
    pub(super) fn start(mut pipe: impl AsyncRead + Unpin + Send + 'static) -> Self {
        let tail = Arc::new(Mutex::new(VecDeque::with_capacity(LIMIT)));
        let shared = tail.clone();
        let reader = tokio::spawn(async move {
            let mut bytes = [0; 1024];
            while let Ok(count) = pipe.read(&mut bytes).await {
                if count == 0 {
                    break;
                }
                let mut tail = shared.lock().unwrap();
                for byte in &bytes[..count] {
                    if tail.len() == LIMIT {
                        tail.pop_front();
                    }
                    tail.push_back(*byte);
                }
            }
        });
        Self { tail, reader }
    }
    pub(super) async fn context(&mut self, error: impl std::fmt::Display) -> String {
        // Error paths only. A still-live worker must not delay cancellation or
        // source retirement indefinitely, and may keep stderr open on failure.
        if !self.reader.is_finished() {
            let _ = tokio::time::timeout(Duration::from_millis(200), &mut self.reader).await;
        }
        let bytes: Vec<_> = self.tail.lock().unwrap().iter().copied().collect();
        let text = String::from_utf8_lossy(&bytes);
        let text = text.trim();
        if text.is_empty() {
            error.to_string()
        } else {
            format!("{error}; native media worker: {text}")
        }
    }
}
impl Drop for Diagnostics {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn large_diagnostic_stream_is_drained_and_retains_final_exception() {
        let (mut writer, pipe) = tokio::io::duplex(64);
        let mut diagnostics = Diagnostics::start(pipe);
        writer.write_all(&vec![b'x'; 64 * 1024]).await.unwrap();
        writer.write_all(b"\nCapture unavailable\n").await.unwrap();
        drop(writer);
        let message = diagnostics.context("unexpected EOF").await;
        assert!(message.starts_with("unexpected EOF; native media worker: "));
        assert!(message.ends_with("Capture unavailable"));
        assert_eq!(diagnostics.tail.lock().unwrap().len(), LIMIT);
        // Re-reading an already finished diagnostic must not poll a completed
        // JoinHandle or overwrite the first captured cause.
        assert_eq!(diagnostics.context("unexpected EOF").await, message);
    }

    #[tokio::test]
    async fn an_open_diagnostic_pipe_cannot_prevent_failure_reporting() {
        let (mut writer, pipe) = tokio::io::duplex(64);
        let mut diagnostics = Diagnostics::start(pipe);
        writer.write_all(b"Capture failed").await.unwrap();
        let result =
            tokio::time::timeout(Duration::from_secs(2), diagnostics.context("read error"))
                .await
                .unwrap();
        assert!(result.ends_with("Capture failed"));
        drop(diagnostics);
        drop(writer);
    }
}
