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
    operation: FileSystemRequest,
    identity: ExecutionIdentity,
    initial_cancel: bool,
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
    request.operation.validate().map_err(io::Error::other)?;
    if current_identity()?.observation(
        ExecutionMode::User,
        ExecutionEnvironmentSource::NativeAccount,
    )? != request.identity
    {
        return Err(io::Error::other("file execution identity mismatch"));
    }
    let payload = tokio::time::timeout(Duration::from_secs(30), async {
        let mut payload = Vec::with_capacity(request.operation.payload_size as usize);
        while payload.len() < request.operation.payload_size as usize {
            let frame = peer.receive().await?;
            if frame.tag != 5
                || frame.bytes.is_empty()
                || payload.len() + frame.bytes.len() > request.operation.payload_size as usize
            {
                return Err(io::Error::other("invalid file input frame"));
            }
            payload.extend(frame.bytes);
        }
        if !matches!(decode(peer.receive().await?)?, Control::FileSystemEnd) {
            return Err(io::Error::other("unexpected trailing file input"));
        }
        validate_payload(&request.operation, &payload)?;
        Ok(payload)
    })
    .await
    .map_err(io::Error::other)??;
    let (send, mut events) = mpsc::channel(8);
    let engine = FileEngine::worker(send);
    let (cancel, receive) = watch::channel(request.initial_cancel);
    let mut work = AbortOnDrop(tokio::spawn(engine.execute(
        request.operation,
        payload,
        receive,
    )));
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
    coordinator: FileCoordinator,
    mut cancel: watch::Receiver<bool>,
) -> io::Result<(FileSystemReply, Vec<u8>)> {
    validate_payload(&request, &payload)?;
    let identity = prepared.identity().observation(
        ExecutionMode::User,
        ExecutionEnvironmentSource::NativeAccount,
    )?;
    let (mut peer, child) = launch(executable, prepared).await?;
    let mut owned = OwnedWorker { child, coordinator };
    let result = tokio::time::timeout(Duration::from_secs(30 * 60 + 30), async {
        let initial_cancel = *cancel.borrow();
        peer.control(Control::FileSystem { request: Request { operation: request.clone(), identity, initial_cancel } }).await?;
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
                    validate_reply(&snapshot, &request)?;
                    if snapshot.state != "running" { return Err(io::Error::other("invalid file progress state")); }
                    owned.coordinator.record(publication, &snapshot).await?;
                    peer.control(Control::FileSystemAck { accepted: true, error: None }).await?;
                },
                Reply::Finished { snapshot } => {
                    validate_reply(&snapshot, &request)?;
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
fn validate_reply(reply: &FileSystemReply, request: &FileSystemRequest) -> io::Result<()> {
    if reply.request_id != request.request_id
        || reply.path != request.path
        || reply.kind != request.operation.kind()
        || reply.destination.as_deref() != request.operation.destination()
    {
        return Err(io::Error::other("file worker result identity mismatch"));
    }
    Ok(())
}
