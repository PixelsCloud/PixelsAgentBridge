//! Bounded upload credit. Reading acknowledgements and writing payloads run in
//! separate futures so a progress response never adds one RTT per file chunk.
use super::{BridgeError, TransferControl};
use pab_protocol::DeviceTaskResponse;
use pab_transport::{MAX_PAB_MESSAGE_BYTES, PabBiStream, PabConnectionError};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    fs::File,
    io::AsyncReadExt,
    sync::{OwnedSemaphorePermit, Semaphore},
};

const CHUNK: usize = 64 * 1024;
const WINDOW_CHUNKS: usize = 64; // 4 MiB per active stream
static GLOBAL_CREDIT: OnceLock<Arc<Semaphore>> = OnceLock::new(); // 16 MiB process-wide

struct Outstanding {
    end: u64,
    _local: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
}

pub(super) async fn upload(
    stream: PabBiStream,
    file: &mut File,
    offset: u64,
    size: u64,
    digest: &str,
    timeout: Duration,
    control: &TransferControl,
    progress: impl Fn(u64, u64) + Send + Sync,
) -> Result<(), BridgeError> {
    upload_with_budget(
        stream,
        file,
        offset,
        size,
        digest,
        timeout,
        control,
        progress,
        GLOBAL_CREDIT
            .get_or_init(|| Arc::new(Semaphore::new(256)))
            .clone(),
    )
    .await
}

async fn upload_with_budget(
    stream: PabBiStream,
    file: &mut File,
    offset: u64,
    size: u64,
    digest: &str,
    timeout: Duration,
    control: &TransferControl,
    progress: impl Fn(u64, u64) + Send + Sync,
    budget: Arc<Semaphore>,
) -> Result<(), BridgeError> {
    let (mut writer, mut reader) = stream.into_framed();
    let credit = Arc::new(Semaphore::new(WINDOW_CHUNKS));
    let outstanding = Mutex::new(VecDeque::<Outstanding>::new());
    let send = async {
        let mut sent = offset;
        let mut buffer = vec![0; CHUNK];
        while sent < size {
            control.check()?;
            let local = credit
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| invalid("upload credit closed"))?;
            let global = budget
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| invalid("upload budget closed"))?;
            let limit = (size - sent).min(CHUNK as u64) as usize;
            let count = file.read(&mut buffer[..limit]).await?;
            if count == 0 {
                return Err(invalid("source changed during transfer"));
            }
            sent += count as u64;
            // Register credit before the write: the peer can acknowledge a
            // completed frame before the writer is next scheduled locally.
            outstanding
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_back(Outstanding {
                    end: sent,
                    _local: local,
                    _global: global,
                });
            tokio::time::timeout(timeout, writer.write(&buffer[..count], CHUNK))
                .await
                .map_err(|_| PabConnectionError::Timeout)??;
        }
        writer.finish()?;
        Ok::<_, BridgeError>(())
    };
    let receive = async {
        let mut confirmed = offset;
        loop {
            control.check()?;
            // This read is never restarted by a progress tick or credit wakeup.
            // On timeout/error both stream halves are discarded together.
            let bytes = tokio::time::timeout(timeout, reader.read(MAX_PAB_MESSAGE_BYTES))
                .await
                .map_err(|_| PabConnectionError::Timeout)??;
            let response: DeviceTaskResponse =
                serde_json::from_slice(&bytes).map_err(PabConnectionError::from)?;
            match response {
                DeviceTaskResponse::FileProgress { offset: ack } => {
                    let mut queue = outstanding.lock().unwrap_or_else(|e| e.into_inner());
                    if ack <= confirmed
                        || ack > size
                        || !queue.iter().any(|pending| pending.end == ack)
                    {
                        return Err(invalid("invalid upload acknowledgement offset"));
                    }
                    while queue.front().is_some_and(|pending| pending.end <= ack) {
                        queue.pop_front();
                    }
                    confirmed = ack;
                    drop(queue);
                    progress(confirmed, size);
                }
                DeviceTaskResponse::FileComplete {
                    size: completed,
                    sha256,
                } if confirmed == size && completed == size && sha256 == digest => return Ok(()),
                DeviceTaskResponse::Error { message, .. } => return Err(invalid(&message)),
                _ => return Err(invalid("unexpected upload acknowledgement or completion")),
            }
        }
    };
    // An error drops both directions and all permits. Completion requires both
    // a fully sent source and the receiver's checksum/publication receipt.
    tokio::try_join!(send, receive).map(|_| ())
}

fn invalid(message: &str) -> BridgeError {
    BridgeError::FileTransfer(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    const TIMEOUT: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn sends_multiple_frames_before_ack_and_restores_all_credit_after_errors() {
        for bad_ack in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let data = vec![39; CHUNK * 3 + 17];
            let path = root.path().join("source");
            tokio::fs::write(&path, &data).await.unwrap();
            let digest = format!("{:x}", Sha256::digest(&data));
            let (a, b, client, server) = super::super::file_transfer::tests::pair().await;
            let stream = client.connection.open_bi(TIMEOUT).await.unwrap();
            let peer = async {
                let mut stream = server.accept_bi(TIMEOUT).await.unwrap();
                let mut received = Vec::new();
                // The old stop-and-wait client deadlocks here after frame one.
                for _ in 0..3 {
                    received.extend(stream.receive_binary_frame(TIMEOUT).await.unwrap());
                }
                assert_eq!(received, data[..CHUNK * 3]);
                if bad_ack {
                    stream
                        .send_json(&DeviceTaskResponse::FileProgress { offset: 1 }, TIMEOUT)
                        .await
                        .unwrap();
                } else {
                    stream
                        .send_frame_json(
                            &DeviceTaskResponse::FileProgress {
                                offset: received.len() as u64,
                            },
                            TIMEOUT,
                        )
                        .await
                        .unwrap();
                    received.extend(stream.receive_binary_frame(TIMEOUT).await.unwrap());
                    assert_eq!(received, data);
                    stream.expect_receive_end(TIMEOUT).await.unwrap();
                    stream
                        .send_frame_json(
                            &DeviceTaskResponse::FileProgress {
                                offset: received.len() as u64,
                            },
                            TIMEOUT,
                        )
                        .await
                        .unwrap();
                    stream
                        .send_json(
                            &DeviceTaskResponse::FileComplete {
                                size: received.len() as u64,
                                sha256: digest.clone(),
                            },
                            TIMEOUT,
                        )
                        .await
                        .unwrap();
                }
            };
            let mut file = File::open(&path).await.unwrap();
            let control = TransferControl::default();
            let budget = Arc::new(Semaphore::new(256));
            let sender = upload_with_budget(
                stream,
                &mut file,
                0,
                data.len() as u64,
                &digest,
                TIMEOUT,
                &control,
                |_, _| {},
                budget.clone(),
            );
            let (result, ()) = tokio::time::timeout(Duration::from_secs(15), async {
                tokio::join!(sender, peer)
            })
            .await
            .unwrap();
            assert_eq!(result.is_ok(), !bad_ack, "{result:?}");
            assert_eq!(budget.available_permits(), 256);
            a.close().await;
            b.close().await;
        }
    }
}
