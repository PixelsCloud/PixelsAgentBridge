//! Streaming file IO in the selected native account. Machine DB and shared
//! path locks remain in the parent. Each message has bounded backpressure.
use super::*;
use crate::task_service::{
    filesystem_engine::{FileCoordination, FileCoordinator},
    transfer_engine::{
        LocalRecorder, TransferEngine, TransferRecord, TransferRecordRequest, TransferRecorder,
        TransferStream, WorkerRecorder,
    },
};
use pab_protocol::{
    DeviceTaskResponse, ExecutionEnvironmentSource, ExecutionIdentity, ExecutionMode, RequestId,
};
use std::path::PathBuf;
use tokio::sync::oneshot;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Operation {
    Upload {
        path: String,
        size: u64,
        sha256: String,
        overwrite: bool,
    },
    Download {
        path: String,
        offset: u64,
        expected_sha256: Option<String>,
    },
    Reconcile {
        path: String,
        size: u64,
        sha256: String,
    },
}
impl Operation {
    fn validate(&self) -> io::Result<()> {
        let path = match self {
            Self::Upload { path, .. }
            | Self::Download { path, .. }
            | Self::Reconcile { path, .. } => path,
        };
        if path.is_empty()
            || path.len() > 4096
            || path.contains('\0')
            || !Path::new(path).is_absolute()
        {
            return Err(io::Error::other("invalid transfer path"));
        }
        match self {
            Self::Upload { size, sha256, .. } | Self::Reconcile { size, sha256, .. }
                if *size > i64::MAX as u64
                    || sha256.len() != 64
                    || !sha256
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) =>
            {
                Err(io::Error::other("invalid upload size or checksum"))
            }
            Self::Download { offset, .. } if *offset > i64::MAX as u64 => {
                Err(io::Error::other("invalid download offset"))
            }
            _ => Ok(()),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    operation: Operation,
    identity: ExecutionIdentity,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Reply {
    Lock {
        id: RequestId,
        key: PathBuf,
    },
    Unlock {
        id: RequestId,
    },
    Record {
        event: TransferRecord,
    },
    Response {
        value: Box<DeviceTaskResponse>,
        end: bool,
    },
    ReadChunk,
    Done {
        error: Option<String>,
    },
}
enum StreamEvent {
    Response {
        value: DeviceTaskResponse,
        end: bool,
        answer: oneshot::Sender<io::Result<()>>,
    },
    Read {
        answer: oneshot::Sender<io::Result<Vec<u8>>>,
    },
    Write {
        bytes: Vec<u8>,
        answer: oneshot::Sender<io::Result<()>>,
    },
}
struct ChildStream(mpsc::Sender<StreamEvent>);
async fn answered<T>(
    result: oneshot::Receiver<io::Result<T>>,
    timeout: Duration,
) -> Result<T, TaskServiceError> {
    tokio::time::timeout(timeout, result)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "transfer parent timed out"))?
        .map_err(|_| io::Error::other("transfer parent disconnected"))?
        .map_err(Into::into)
}
impl TransferStream for ChildStream {
    async fn response(
        &mut self,
        value: &DeviceTaskResponse,
        end: bool,
        timeout: Duration,
    ) -> Result<(), TaskServiceError> {
        let (answer, result) = oneshot::channel();
        self.0
            .send(StreamEvent::Response {
                value: value.clone(),
                end,
                answer,
            })
            .await
            .map_err(|_| io::Error::other("transfer stream closed"))?;
        answered(result, timeout).await
    }
    async fn receive_binary_frame(
        &mut self,
        timeout: Duration,
    ) -> Result<Vec<u8>, TaskServiceError> {
        let (answer, result) = oneshot::channel();
        self.0
            .send(StreamEvent::Read { answer })
            .await
            .map_err(|_| io::Error::other("transfer stream closed"))?;
        answered(result, timeout).await
    }
    async fn send_binary_frame(
        &mut self,
        bytes: &[u8],
        timeout: Duration,
    ) -> Result<(), TaskServiceError> {
        let (answer, result) = oneshot::channel();
        self.0
            .send(StreamEvent::Write {
                bytes: bytes.to_vec(),
                answer,
            })
            .await
            .map_err(|_| io::Error::other("transfer stream closed"))?;
        answered(result, timeout).await
    }
}
async fn ack(peer: &mut Peer) -> io::Result<bool> {
    match decode(
        tokio::time::timeout(Duration::from_secs(60), peer.receive())
            .await
            .map_err(io::Error::other)??,
    )? {
        Control::TransferAck { accepted } => Ok(accepted),
        _ => Err(io::Error::other("unexpected transfer acknowledgement")),
    }
}
fn accepted(value: bool) -> io::Result<()> {
    if value {
        Ok(())
    } else {
        Err(io::Error::other("transfer event rejected"))
    }
}

pub(super) async fn serve(mut peer: Peer, request: Request) -> io::Result<()> {
    request.operation.validate()?;
    if current_identity()?.observation(
        ExecutionMode::User,
        ExecutionEnvironmentSource::NativeAccount,
    )? != request.identity
    {
        return Err(io::Error::other("transfer worker identity mismatch"));
    }
    let (send_locks, mut locks) = mpsc::channel(8);
    let (send_records, mut records) = mpsc::channel(8);
    let (send_stream, mut stream) = mpsc::channel(8);
    let engine = TransferEngine::worker(
        WorkerRecorder(send_records),
        send_locks,
        &serde_json::to_vec(&request.identity)?,
    );
    let mut work = AbortOnDrop(tokio::spawn(async move {
        let mut stream = ChildStream(send_stream);
        let timeout = Duration::from_secs(60);
        match request.operation {
            Operation::Upload {
                path,
                size,
                sha256,
                overwrite,
            } => {
                engine
                    .upload(&mut stream, timeout, &path, size, &sha256, overwrite)
                    .await
            }
            Operation::Download {
                path,
                offset,
                expected_sha256,
            } => {
                engine
                    .download(
                        &mut stream,
                        timeout,
                        &path,
                        offset,
                        expected_sha256.as_deref(),
                    )
                    .await
            }
            Operation::Reconcile { path, size, sha256 } => {
                if !engine.verify_publication(&path, size, &sha256).await? {
                    return Err(io::Error::other(
                        "original user could not confirm transfer publication",
                    )
                    .into());
                }
                stream
                    .response(
                        &DeviceTaskResponse::FileComplete { size, sha256 },
                        true,
                        timeout,
                    )
                    .await
            }
        }
    }));
    loop {
        tokio::select! {
            biased;
            Some(event) = locks.recv() => match event {
                FileCoordination::Lock { id, key, answer } => {
                    peer.control(Control::TransferReply { reply: Reply::Lock { id, key } }).await?;
                    let _ = answer.send(Ok(ack(&mut peer).await?));
                },
                FileCoordination::Unlock { id } => {
                    peer.control(Control::TransferReply { reply: Reply::Unlock { id } }).await?;
                },
                _ => return Err(io::Error::other("unexpected transfer file record")),
            },
            Some(TransferRecordRequest { event, answer }) = records.recv() => {
                peer.control(Control::TransferReply { reply: Reply::Record { event } }).await?;
                let _ = answer.send(accepted(ack(&mut peer).await?));
            },
            Some(event) = stream.recv() => match event {
                StreamEvent::Response { value, end, answer } => {
                    peer.control(Control::TransferReply { reply: Reply::Response { value: Box::new(value), end } }).await?;
                    let _ = answer.send(accepted(ack(&mut peer).await?));
                },
                StreamEvent::Read { answer } => {
                    peer.control(Control::TransferReply { reply: Reply::ReadChunk }).await?;
                    let frame = tokio::time::timeout(Duration::from_secs(60), peer.receive()).await.map_err(io::Error::other)??;
                    if frame.tag != 7 || frame.bytes.is_empty() { return Err(io::Error::other("invalid transfer input chunk")); }
                    let _ = answer.send(Ok(frame.bytes));
                },
                StreamEvent::Write { bytes, answer } => {
                    peer.send(8, &bytes).await?;
                    let _ = answer.send(accepted(ack(&mut peer).await?));
                },
            },
            result = &mut work.0 => {
                let error = result.map_err(io::Error::other)?.err().map(|e| e.to_string().chars().take(2048).collect());
                peer.control(Control::TransferReply { reply: Reply::Done { error } }).await?;
                return Ok(());
            },
            frame = peer.receive() => {
                let _ = frame?;
                return Err(io::Error::other("unexpected transfer worker input"));
            }
        }
    }
}

// Drop native child before shared locks if the parent future is cancelled.
struct OwnedWorker {
    child: pab_os_control::execution::UserProcess,
    coordinator: FileCoordinator,
}
/// The caller accepts the original record before launching and settles errors
/// afterwards. The engine persists publication/completion before final receipt.
pub(crate) async fn execute(
    executable: &Path,
    prepared: PreparedUser,
    operation: Operation,
    records: LocalRecorder,
    coordinator: FileCoordinator,
    stream: &mut impl TransferStream,
    mut cancel: watch::Receiver<bool>,
) -> io::Result<()> {
    operation.validate()?;
    let identity = prepared.identity().observation(
        ExecutionMode::User,
        ExecutionEnvironmentSource::NativeAccount,
    )?;
    let (mut peer, child) = launch(executable, prepared).await?;
    let mut owned = OwnedWorker { child, coordinator };
    let timeout = Duration::from_secs(60);
    let mut terminal = None;
    let result = async {
        if *cancel.borrow() { return Err(io::Error::new(io::ErrorKind::Interrupted, "transfer cancelled before execution")); }
        peer.control(Control::Transfer { request: Request { operation: operation.clone(), identity } }).await?;
        let run = async {
            loop {
                let frame = tokio::time::timeout(timeout, peer.receive()).await.map_err(io::Error::other)??;
                if frame.tag == 8 {
                    if !matches!(operation, Operation::Download { .. }) || frame.bytes.is_empty() || terminal.is_some() { return Err(io::Error::other("unexpected transfer output chunk")); }
                    stream.send_binary_frame(&frame.bytes, timeout).await.map_err(io::Error::other)?;
                    peer.control(Control::TransferAck { accepted: true }).await?;
                    continue;
                }
                let Control::TransferReply { reply } = decode(frame)? else { return Err(io::Error::other("unexpected transfer worker reply")); };
                match reply {
                    Reply::Lock { id, key } => {
                        let accepted = owned.coordinator.lock(id, key)?;
                        peer.control(Control::TransferAck { accepted }).await?;
                    },
                    Reply::Unlock { id } => owned.coordinator.unlock(id)?,
                    Reply::Record { event } => {
                        if matches!(operation, Operation::Reconcile { .. }) { return Err(io::Error::other("reconciliation cannot write records")); }
                        records.record(event).await?;
                        peer.control(Control::TransferAck { accepted: true }).await?;
                    },
                    Reply::ReadChunk => {
                        if !matches!(operation, Operation::Upload { .. }) || terminal.is_some() { return Err(io::Error::other("unexpected transfer input request")); }
                        let bytes = stream.receive_binary_frame(timeout).await.map_err(io::Error::other)?;
                        if bytes.is_empty() || bytes.len() > BINARY_LIMIT { return Err(io::Error::other("invalid upload input chunk")); }
                        peer.send(7, &bytes).await?;
                    },
                    Reply::Response { value, end } => {
                        if terminal.is_some() || !matches!(&*value, DeviceTaskResponse::FileReady { .. } | DeviceTaskResponse::FileProgress { .. } | DeviceTaskResponse::FileComplete { .. } | DeviceTaskResponse::Error { .. }) { return Err(io::Error::other("invalid transfer response")); }
                        if end { terminal = Some(*value); }
                        else { stream.response(&value, false, timeout).await.map_err(io::Error::other)?; }
                        peer.control(Control::TransferAck { accepted: true }).await?;
                    },
                    Reply::Done { error } => return error.map_or(Ok(()), |error| Err(io::Error::other(error))),
                }
            }
        };
        tokio::select! {
            result = run => result,
            _ = async { if cancel.wait_for(|value| *value).await.is_err() { std::future::pending::<()>().await; } } => Err(io::Error::new(io::ErrorKind::Interrupted, "transfer cancellation requested")),
        }
    }.await;
    drop(peer);
    let cleanup = tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            if let Some(code) = owned.child.try_wait()? {
                if code != 0 && result.is_ok() {
                    return Err(io::Error::other(format!("transfer worker exited {code}")));
                }
                return owned.child.terminate();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(io::Error::other)
    .and_then(|v| v);
    if cleanup.is_err() {
        let _ = owned.child.terminate();
    }
    drop(owned);
    cleanup?;
    if let Some(value) = terminal {
        stream
            .response(&value, true, timeout)
            .await
            .map_err(io::Error::other)?;
    } else if result.is_ok() {
        return Err(io::Error::other("transfer finished without receipt"));
    }
    result
}
