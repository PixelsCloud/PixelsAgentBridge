use super::*;
use crate::task_service::transfer_tests::{actor, service};
use std::collections::VecDeque;

#[derive(Default)]
struct MemoryStream {
    input: VecDeque<Vec<u8>>,
    terminal: bool,
    sent: Vec<u8>,
}
impl TransferStream for MemoryStream {
    async fn response(
        &mut self,
        _: &DeviceTaskResponse,
        end: bool,
        _: Duration,
    ) -> Result<(), TaskServiceError> {
        self.terminal |= end;
        Ok(())
    }
    async fn receive_binary_frame(&mut self, _: Duration) -> Result<Vec<u8>, TaskServiceError> {
        self.input.pop_front().ok_or_else(|| {
            io::Error::new(io::ErrorKind::UnexpectedEof, "fixture disconnected").into()
        })
    }
    async fn send_binary_frame(
        &mut self,
        bytes: &[u8],
        _: Duration,
    ) -> Result<(), TaskServiceError> {
        assert!(!bytes.is_empty() && bytes.len() <= 64 * 1024);
        self.sent.extend_from_slice(bytes);
        Ok(())
    }
}

#[tokio::test]
async fn transfer_publication_requires_parent_ack_and_lost_completion_stays_reconcilable() {
    // Same engine as production, with the future user worker's bounded channels.
    // None succeeds; 0 drops publication permission; 1 loses completion ack.
    for disconnect in [None, Some(0), Some(1)] {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(dir.path()).await;
        let destination = dir.path().join("中文 transfer.bin");
        tokio::fs::write(&destination, b"original").await.unwrap();
        let bytes: Vec<u8> = (0..131071).map(|n| (n % 251) as u8).collect();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let id = RequestId::new();
        svc.store
            .start_transfer(
                id,
                actor(),
                "receive",
                destination.to_str().unwrap(),
                bytes.len() as u64,
                Some(&hash),
            )
            .await
            .unwrap();
        let recorder = LocalRecorder {
            store: svc.store.clone(),
            request_id: id,
        };
        let mut coordinator = svc.file_coordinator();
        let (send_locks, mut locks) = mpsc::channel(8);
        let (send_records, mut records) = mpsc::channel(8);
        let engine =
            TransferEngine::worker(WorkerRecorder(send_records), send_locks, b"fixture-user");
        let path = destination.clone();
        let expected = bytes.clone();
        let mut worker = tokio::spawn(async move {
            let mut stream = MemoryStream {
                input: expected.chunks(65536).map(Vec::from).collect(),
                ..Default::default()
            };
            let result = engine
                .upload(
                    &mut stream,
                    Duration::from_secs(5),
                    path.to_str().unwrap(),
                    expected.len() as u64,
                    &hash,
                    true,
                )
                .await;
            (result, stream.terminal)
        });
        let outcome = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    value = &mut worker => break value.unwrap(),
                    Some(event) = locks.recv() => match event {
                        FileCoordination::Lock { id, key, answer } => { let _ = answer.send(coordinator.lock(id, key)); },
                        FileCoordination::Unlock { id } => { coordinator.unlock(id).unwrap(); },
                        _ => panic!("unexpected filesystem record"),
                    },
                    Some(TransferRecordRequest { event, answer }) = records.recv() => {
                        if matches!(event, TransferRecord::BeginPublication) {
                            assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"original");
                            assert!(svc.upload_locks.try_acquire(&destination).await.unwrap().is_none());
                            if disconnect == Some(0) { drop(answer); continue; }
                        }
                        if matches!(event, TransferRecord::Completed) {
                            assert_eq!(tokio::fs::read(&destination).await.unwrap(), bytes);
                            if disconnect == Some(1) { drop(answer); continue; }
                        }
                        let _ = answer.send(recorder.record(event).await);
                    }
                }
            }
        }).await.unwrap();
        assert_eq!(outcome.0.is_ok(), disconnect.is_none());
        assert_eq!(outcome.1, disconnect.is_none());
        drop(coordinator); // Parent keeps the path locked through worker exit.
        assert!(
            svc.upload_locks
                .try_acquire(&destination)
                .await
                .unwrap()
                .is_some()
        );
        let snapshot = svc.store.get_transfer(actor(), id).await.unwrap();
        if disconnect == Some(0) {
            assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"original");
            assert_eq!(snapshot.state, "running");
        } else {
            assert_eq!(snapshot.state, "completed");
            assert_eq!(snapshot.published, Some(true));
        }
    }
}

#[tokio::test]
async fn transfer_engine_rejects_empty_or_oversized_adapter_chunks_without_publication() {
    for input in [vec![], vec![1; MAX_BINARY_FRAME_BYTES + 1], vec![1; 4]] {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(dir.path()).await;
        let path = dir.path().join("result.bin");
        tokio::fs::write(&path, b"original").await.unwrap();
        let id = RequestId::new();
        let hash = format!("{:x}", Sha256::digest([1; 3]));
        svc.store
            .start_transfer(
                id,
                actor(),
                "receive",
                path.to_str().unwrap(),
                3,
                Some(&hash),
            )
            .await
            .unwrap();
        let mut stream = MemoryStream {
            input: [input].into(),
            ..Default::default()
        };
        assert!(
            TransferEngine::local(&svc.store, id, &svc.upload_locks)
                .upload(
                    &mut stream,
                    Duration::from_secs(5),
                    path.to_str().unwrap(),
                    3,
                    &hash,
                    true
                )
                .await
                .is_err()
        );
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"original");
        assert!(stream.terminal);
        assert_eq!(svc.store.get_transfer(actor(), id).await.unwrap().offset, 0);
    }
}

#[tokio::test]
async fn transfer_engine_streams_resumed_download_and_records_the_whole_hash() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let path = dir.path().join("input.bin");
    let bytes: Vec<u8> = (0..196607).map(|n| (n % 251) as u8).collect();
    tokio::fs::write(&path, &bytes).await.unwrap();
    let id = RequestId::new();
    svc.store
        .start_transfer(id, actor(), "send", path.to_str().unwrap(), 0, None)
        .await
        .unwrap();
    let mut stream = MemoryStream::default();
    TransferEngine::local(&svc.store, id, &svc.upload_locks)
        .download(
            &mut stream,
            Duration::from_secs(5),
            path.to_str().unwrap(),
            65536,
            None,
        )
        .await
        .unwrap();
    assert_eq!(stream.sent, bytes[65536..]);
    assert!(stream.terminal);
    let snapshot = svc.store.get_transfer(actor(), id).await.unwrap();
    assert_eq!(snapshot.state, "completed");
    assert_eq!(
        snapshot.sha256,
        Some(format!("{:x}", Sha256::digest(&bytes)))
    );
    assert_eq!(snapshot.offset, bytes.len() as u64);
}
