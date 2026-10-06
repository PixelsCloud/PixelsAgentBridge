//! One operation per native user worker. File bytes stay binary; the parent
//! owns durable progress and the lock namespace shared with service operations.
use super::*;
use crate::task_service::filesystem_engine::{FileCoordination, FileCoordinator, FileEngine};
use pab_protocol::{
    ExecutionEnvironmentSource, ExecutionIdentity, ExecutionMode, FileSystemReply,
    FileSystemRequest, RequestId,
};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    operation: Operation,
    identity: ExecutionIdentity,
    context: pab_protocol::ExecutionContext,
    initial_cancel: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Execute { request: FileSystemRequest },
    Reconcile { original: FileSystemReply },
}
impl Operation {
    fn validate(&self) -> io::Result<()> {
        match self {
            Self::Execute { request } => request.validate().map_err(io::Error::other),
            Self::Reconcile { original }
                if original.state == "unconfirmed"
                    && matches!(original.kind.as_str(), "file_write" | "file_patch") =>
            {
                Ok(())
            }
            _ => Err(io::Error::other("invalid file reconciliation")),
        }
    }
    fn payload_size(&self) -> usize {
        match self {
            Self::Execute { request } => request.payload_size as usize,
            _ => 0,
        }
    }
    fn validate_payload(&self, bytes: &[u8]) -> io::Result<()> {
        self.validate()?;
        match self {
            Self::Execute { request } => validate_payload(request, bytes),
            Self::Reconcile { .. } if bytes.is_empty() => Ok(()),
            _ => Err(io::Error::other("unexpected reconcile payload")),
        }
    }
    fn validate_reply(&self, reply: &FileSystemReply) -> io::Result<()> {
        let expected = match self {
            Self::Execute { request } => FileSystemReply::pending(request),
            Self::Reconcile { original } => original.clone(),
        };
        if reply.request_id != expected.request_id
            || reply.path != expected.path
            || reply.kind != expected.kind
            || reply.destination != expected.destination
        {
            return Err(io::Error::other("file result request mismatch"));
        }
        Ok(())
    }
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
        publication: bool,
        snapshot: FileSystemReply,
    },
    Finished {
        snapshot: FileSystemReply,
    },
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn validate_payload(request: &FileSystemRequest, payload: &[u8]) -> io::Result<()> {
    request.validate().map_err(io::Error::other)?;
    if payload.len() != request.payload_size as usize
        || (request.operation.has_payload()
            && request.payload_sha256.as_deref() != Some(digest(payload).as_str()))
    {
        return Err(io::Error::other(
            "file worker payload size or hash mismatch",
        ));
    }
    Ok(())
}
pub(super) async fn serve(mut peer: Peer, request: Request) -> io::Result<()> {
    request.operation.validate()?;
    if current_identity()?.observation(
        ExecutionMode::User,
        ExecutionEnvironmentSource::NativeAccount,
    )? != request.identity
        || request.context.identity.as_ref() != Some(&request.identity)
    {
        return Err(io::Error::other("file execution identity mismatch"));
    }
    let payload = tokio::time::timeout(Duration::from_secs(30), async {
        let mut payload = Vec::with_capacity(request.operation.payload_size());
        while payload.len() < request.operation.payload_size() {
            let frame = peer.receive().await?;
            if frame.tag != 5
                || frame.bytes.is_empty()
                || payload.len() + frame.bytes.len() > request.operation.payload_size()
            {
                return Err(io::Error::other("invalid file input frame"));
            }
            payload.extend(frame.bytes);
        }
        if !matches!(decode(peer.receive().await?)?, Control::FileSystemEnd) {
            return Err(io::Error::other("unexpected trailing file input"));
        }
        request.operation.validate_payload(&payload)?;
        Ok(payload)
    })
    .await
    .map_err(io::Error::other)??;
    let (send, mut events) = mpsc::channel(8);
    let engine = FileEngine::worker(send);
    let (cancel, receive) = watch::channel(request.initial_cancel);
    let mut work = AbortOnDrop(tokio::spawn(async move {
        match request.operation {
            Operation::Execute { request: operation } => {
                engine
                    .execute(operation, payload, Some(request.context), receive)
                    .await
            }
            Operation::Reconcile { original } => (engine.reconcile(original).await, Vec::new()),
        }
    }));
    let result = async {
        loop {
            tokio::select! {
                frame = peer.receive() => match decode(frame?)? {
                    Control::Cancel { .. } => { let _ = cancel.send(true); },
                    _ => return Err(io::Error::other("unexpected file worker request")),
                },
                event = events.recv() => match event {
                    Some(FileCoordination::Unlock { id }) => {
                        peer.control(Control::FileSystemReply { reply: Reply::Unlock { id } }).await?;
                    },
                    Some(event) => {
                        match event {
                            FileCoordination::Lock { id, key, answer } => {
                                peer.control(Control::FileSystemReply { reply: Reply::Lock { id, key } }).await?;
                                let _ = answer.send(ack(&mut peer, &cancel).await?);
                            },
                            FileCoordination::Record { publication, reply, answer } => {
                                peer.control(Control::FileSystemReply { reply: Reply::Record { publication, snapshot: reply } }).await?;
                                let result = ack(&mut peer, &cancel).await?.and_then(|accepted| {
                                    if accepted { Ok(()) } else { Err(io::Error::other("file record not accepted")) }
                                });
                                let _ = answer.send(result);
                            },
                            FileCoordination::Unlock { .. } => unreachable!(),
                        }
                    },
                    None => {
                        let (mut snapshot, bytes) = (&mut work.0).await.map_err(io::Error::other)?;
                        snapshot.data_size = bytes.len() as u32;
                        snapshot.data_sha256 = (!bytes.is_empty() || snapshot.kind == "file_read" && snapshot.state == "completed").then(|| digest(&bytes));
                        peer.control(Control::FileSystemReply { reply: Reply::Finished { snapshot } }).await?;
                        for chunk in bytes.chunks(64 * 1024) { peer.send(6, chunk).await?; }
                        peer.control(Control::FileSystemEnd).await?;
                        return Ok(());
                    }
                }
            }
        }
    }.await;
    if result.is_err() {
        // Drop pending acknowledgements so a staging writer can unwind. Bulk
        // work cooperatively stops before the process exit fallback takes over.
        drop(events);
        let _ = cancel.send(true);
        if !work.0.is_finished() {
            let _ = tokio::time::timeout(Duration::from_secs(5), &mut work.0).await;
        }
    }
    result
}
async fn ack(peer: &mut Peer, cancel: &watch::Sender<bool>) -> io::Result<io::Result<bool>> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match decode(peer.receive().await?)? {
                Control::FileSystemAck { accepted, error } => {
                    return Ok(error.map_or(Ok(accepted), |error| Err(io::Error::other(error))));
                }
                Control::Cancel { .. } => {
                    let _ = cancel.send(true);
                }
                _ => return Err(io::Error::other("unexpected file acknowledgement")),
            }
        }
    })
    .await
    .map_err(io::Error::other)?
}

// Field drop order also preserves locks when the parent's async task aborts.
struct OwnedWorker {
    child: pab_os_control::execution::UserProcess,
    coordinator: FileCoordinator,
}
pub(crate) async fn execute(
    executable: &Path,
    prepared: PreparedUser,
    request: FileSystemRequest,
    payload: Vec<u8>,
    context: pab_protocol::ExecutionContext,
    coordinator: FileCoordinator,
    cancel: watch::Receiver<bool>,
) -> io::Result<(FileSystemReply, Vec<u8>)> {
    operate(
        executable,
        prepared,
        Operation::Execute { request },
        payload,
        context,
        coordinator,
        cancel,
    )
    .await
}
pub(crate) async fn reconcile(
    executable: &Path,
    prepared: PreparedUser,
    original: FileSystemReply,
    coordinator: FileCoordinator,
) -> io::Result<FileSystemReply> {
    let context = original
        .execution_context
        .clone()
        .ok_or_else(|| io::Error::other("missing original file identity"))?;
    let (_send, cancel) = watch::channel(false);
    operate(
        executable,
        prepared,
        Operation::Reconcile { original },
        Vec::new(),
        context,
        coordinator,
        cancel,
    )
    .await
    .map(|(reply, _)| reply)
}
async fn operate(
    executable: &Path,
    prepared: PreparedUser,
    request: Operation,
    payload: Vec<u8>,
    context: pab_protocol::ExecutionContext,
    coordinator: FileCoordinator,
    mut cancel: watch::Receiver<bool>,
) -> io::Result<(FileSystemReply, Vec<u8>)> {
    request.validate_payload(&payload)?;
    let identity = prepared.identity().observation(
        ExecutionMode::User,
        ExecutionEnvironmentSource::NativeAccount,
    )?;
    let (mut peer, child) = launch(executable, prepared).await?;
    let mut owned = OwnedWorker { child, coordinator };
    let timeout = if matches!(request, Operation::Reconcile { .. }) {
        Duration::from_secs(15)
    } else {
        Duration::from_secs(30 * 60 + 30)
    };
    let result = tokio::time::timeout(timeout, async {
        let initial_cancel = *cancel.borrow();
        peer.control(Control::FileSystem { request: Request { operation: request.clone(), identity, context: context.clone(), initial_cancel } }).await?;
        for chunk in payload.chunks(64 * 1024) { peer.send(5, chunk).await?; }
        peer.control(Control::FileSystemEnd).await?;
        let mut cancel_at = None;
        if *cancel.borrow() {
            peer.control(Control::Cancel { reason: "file cancellation requested".into() }).await?;
            cancel_at = Some(tokio::time::Instant::now() + Duration::from_secs(10));
        }
        let mut cancel_open = true;
        loop {
            let wait_cancel = async { match cancel_at { Some(at) => tokio::time::sleep_until(at).await, None => std::future::pending::<()>().await } };
            let frame = tokio::select! {
                frame = peer.receive() => frame?,
                changed = cancel.changed(), if cancel_open && cancel_at.is_none() => {
                    if changed.is_err() { cancel_open = false; }
                    else if *cancel.borrow() {
                        peer.control(Control::Cancel { reason: "file cancellation requested".into() }).await?;
                        cancel_at = Some(tokio::time::Instant::now() + Duration::from_secs(10));
                    }
                    continue;
                },
                _ = wait_cancel => return Err(io::Error::new(io::ErrorKind::TimedOut, "file worker cancellation unconfirmed")),
            };
            let Control::FileSystemReply { reply } = decode(frame)? else { return Err(io::Error::other("unexpected file worker response")); };
            match reply {
                Reply::Lock { id, key } => {
                    let accepted = owned.coordinator.lock(id, key)?;
                    peer.control(Control::FileSystemAck { accepted, error: None }).await?;
                },
                Reply::Unlock { id } => owned.coordinator.unlock(id)?,
                Reply::Record { publication, snapshot } => {
                    if matches!(request, Operation::Reconcile { .. }) { return Err(io::Error::other("unexpected reconcile write record")); }
                    request.validate_reply(&snapshot)?;
                    if snapshot.execution_context.as_ref() != Some(&context) { return Err(io::Error::other("file progress execution identity mismatch")); }
                    if snapshot.state != "running" { return Err(io::Error::other("invalid file progress state")); }
                    owned.coordinator.record(publication, &snapshot).await?;
                    peer.control(Control::FileSystemAck { accepted: true, error: None }).await?;
                },
                Reply::Finished { snapshot } => {
                    request.validate_reply(&snapshot)?;
                    if snapshot.execution_context.as_ref() != Some(&context) { return Err(io::Error::other("file result execution identity mismatch")); }
                    if !["completed", "failed", "cancelled", "unconfirmed"].contains(&snapshot.state.as_str()) || snapshot.data_size as usize > pab_protocol::MAX_TEXT_READ_BYTES {
                        return Err(io::Error::other("invalid file terminal result"));
                    }
                    let mut bytes = Vec::with_capacity(snapshot.data_size as usize);
                    while bytes.len() < snapshot.data_size as usize {
                        let frame = peer.receive().await?;
                        if frame.tag != 6 || frame.bytes.is_empty() || bytes.len() + frame.bytes.len() > snapshot.data_size as usize { return Err(io::Error::other("invalid file output frame")); }
                        bytes.extend(frame.bytes);
                    }
                    if !matches!(decode(peer.receive().await?)?, Control::FileSystemEnd) || snapshot.data_sha256.as_ref().is_some_and(|hash| *hash != digest(&bytes)) || !bytes.is_empty() && snapshot.data_sha256.is_none() {
                        return Err(io::Error::other("file output hash or end marker mismatch"));
                    }
                    return Ok((snapshot, bytes));
                },
            }
        }
    }).await.map_err(io::Error::other).and_then(|v| v);
    drop(peer);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    let cleanup = async {
        loop {
            if let Some(code) = owned.child.try_wait()? {
                if code != 0 && result.is_ok() {
                    return Err(io::Error::other(format!("file worker exited {code}")));
                }
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "file worker exit unconfirmed",
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        owned.child.terminate()
    }
    .await;
    if cleanup.is_err() {
        let _ = owned.child.terminate();
    }
    drop(owned);
    cleanup?;
    result
}
