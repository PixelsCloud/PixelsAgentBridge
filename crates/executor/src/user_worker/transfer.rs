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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_path: Option<pab_protocol::NativeFilePath>,
    #[serde(default)]
    attributes: Attributes,
}

#[derive(Clone,Default,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Attributes {
    pub metadata: Option<pab_protocol::FileTransferMetadata>,
    pub source_constraint: Option<pab_protocol::FileTransferSourceConstraint>,
    #[serde(default)]
    pub destination_parent:Option<pab_protocol::FileTransferParentConstraint>,
}
impl Attributes {
    fn validate(&self,operation:&Operation)->io::Result<()> {
        if let Some(parent)=&self.destination_parent {
            parent.validate().map_err(io::Error::other)?;
            if matches!(operation,Operation::Download {..}) {return Err(io::Error::other("remote source cannot use receiver parent constraint"));}
        }
        if let Some(metadata)=&self.metadata {metadata.validate().map_err(io::Error::other)?;}
        if let Some(source)=&self.source_constraint {
            source.validate().map_err(io::Error::other)?;
            match operation {
                Operation::Upload {size,..} if *size!=source.size.0=>return Err(io::Error::other("upload source size mismatch")),
                Operation::Download {offset,..} if *offset>source.size.0=>return Err(io::Error::other("source offset exceeds size")),
                _=>{},
            }
        }
        if matches!(operation,Operation::Reconcile {..}) && self.source_constraint.is_some() {
            return Err(io::Error::other("publication reconciliation has no source precondition"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod attribute_tests {
    use super::*;
    #[test]
    fn attributes_roundtrip_and_reject_privilege_or_inconsistent_sources() {
        use pab_protocol::{FileByteCount,FileTransferMetadata,FileTransferSourceConstraint};
        let upload=Operation::Upload {path:"fixture".into(),size:8,sha256:"a".repeat(64),overwrite:false};
        let mut attributes=Attributes {destination_parent:None,metadata:Some(FileTransferMetadata {modified_at_unix_ms:Some(123),unix_mode:Some(0o640)}),source_constraint:Some(FileTransferSourceConstraint {size:FileByteCount(8),modified_at_unix_ms:Some(123)})};
        attributes.validate(&upload).unwrap();
        let decoded:Attributes=serde_json::from_slice(&serde_json::to_vec(&attributes).unwrap()).unwrap();
        assert_eq!(decoded.metadata,attributes.metadata);assert_eq!(decoded.source_constraint,attributes.source_constraint);
        attributes.metadata.as_mut().unwrap().unix_mode=Some(0o4755);assert!(attributes.validate(&upload).is_err());
        attributes.metadata=None;attributes.source_constraint.as_mut().unwrap().size=FileByteCount(9);assert!(attributes.validate(&upload).is_err());
        assert!(attributes.validate(&Operation::Download {path:"fixture".into(),offset:10,expected_sha256:None}).is_err());
        assert!(attributes.validate(&Operation::Reconcile {path:"fixture".into(),size:9,sha256:"a".repeat(64)}).is_err());
        attributes.source_constraint=None;attributes.metadata=Some(FileTransferMetadata {modified_at_unix_ms:Some(123),unix_mode:Some(0o640)});
        attributes.validate(&Operation::Reconcile {path:"fixture".into(),size:9,sha256:"a".repeat(64)}).unwrap();
        attributes.destination_parent=Some(pab_protocol::FileTransferParentConstraint {identity:"volume:123".into()});
        attributes.validate(&upload).unwrap();attributes.validate(&Operation::Reconcile {path:"fixture".into(),size:9,sha256:"a".repeat(64)}).unwrap();
        assert!(attributes.validate(&Operation::Download {path:"fixture".into(),offset:0,expected_sha256:None}).is_err());
        let decoded:Attributes=serde_json::from_slice(&serde_json::to_vec(&attributes).unwrap()).unwrap();assert_eq!(decoded.destination_parent,attributes.destination_parent);
        attributes.destination_parent.as_mut().unwrap().identity="../replacement".into();assert!(attributes.validate(&upload).is_err());
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Reply {
    End,
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
    End {answer:oneshot::Sender<io::Result<()>>},
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
    async fn end(&mut self,timeout:Duration)->Result<(),TaskServiceError> {
        let (answer,result)=oneshot::channel();
        self.0.send(StreamEvent::End {answer}).await.map_err(|_|io::Error::other("transfer stream closed"))?;
        answered(result,timeout).await
    }
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
    request.attributes.validate(&request.operation)?;
    let label = match &request.operation {
        Operation::Upload { path, .. }
        | Operation::Download { path, .. }
        | Operation::Reconcile { path, .. } => path,
    };
    let operational_path = match &request.native_path {
        Some(path) => {
            if path.display_label() != *label {
                return Err(io::Error::other("native transfer label mismatch"));
            }
            pab_file_manager::decode_path(path).map_err(io::Error::other)?
        }
        None => PathBuf::from(label),
    };
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
    ).with_parent(request.attributes.destination_parent).with_metadata(request.attributes.metadata).with_source_constraint(request.attributes.source_constraint);
    let mut work = AbortOnDrop(tokio::spawn(async move {
        let mut stream = ChildStream(send_stream);
        let timeout = Duration::from_secs(60);
        match request.operation {
            Operation::Upload {
                path: _,
                size,
                sha256,
                overwrite,
            } => {
                engine
                    .upload(
                        &mut stream,
                        timeout,
                        &operational_path,
                        size,
                        &sha256,
                        overwrite,
                    )
                    .await
            }
            Operation::Download {
                path: _,
                offset,
                expected_sha256,
            } => {
                engine
                    .download(
                        &mut stream,
                        timeout,
                        &operational_path,
                        offset,
                        expected_sha256.as_deref(),
                    )
                    .await
            }
            Operation::Reconcile {
                path: _,
                size,
                sha256,
            } => {
                if !engine
                    .verify_publication(&operational_path, size, &sha256)
                    .await?
                {
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
                StreamEvent::End {answer}=>{
                    peer.control(Control::TransferReply {reply:Reply::End}).await?;
                    let _=answer.send(accepted(ack(&mut peer).await?));
                },
                StreamEvent::Response { value, end, answer } => {
                    peer.control(Control::TransferReply { reply: Reply::Response { value: Box::new(value), end } }).await?;
                    let _ = answer.send(accepted(ack(&mut peer).await?));
                },
                StreamEvent::Read { answer } => {
                    peer.control(Control::TransferReply { reply: Reply::ReadChunk }).await?;
                    let frame = tokio::time::timeout(Duration::from_secs(60), peer.receive()).await.map_err(io::Error::other)??;
                    if frame.tag==9 && frame.bytes.is_empty(){let _=answer.send(Err(io::Error::new(io::ErrorKind::Interrupted,"transfer input interrupted")));}
                    else {if frame.tag != 7 || frame.bytes.is_empty() { return Err(io::Error::other("invalid transfer input chunk")); }let _=answer.send(Ok(frame.bytes));}
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
#[cfg(test)]
pub(crate) async fn execute(
    executable: &Path,
    prepared: PreparedUser,
    operation: Operation,
    records: LocalRecorder,
    coordinator: FileCoordinator,
    stream: &mut impl TransferStream,
    cancel: watch::Receiver<bool>,
) -> io::Result<()> {
    execute_native(
        executable,
        prepared,
        operation,
        None,
        Attributes::default(),
        records,
        coordinator,
        stream,
        cancel,
    )
    .await
}

pub(crate) async fn execute_native(
    executable: &Path,
    prepared: PreparedUser,
    operation: Operation,
    native_path: Option<pab_protocol::NativeFilePath>,
    attributes: Attributes,
    records: LocalRecorder,
    coordinator: FileCoordinator,
    stream: &mut impl TransferStream,
    mut cancel: watch::Receiver<bool>,
) -> io::Result<()> {
    operation.validate()?;
    attributes.validate(&operation)?;
    let identity = prepared.identity().observation(
        ExecutionMode::User,
        ExecutionEnvironmentSource::NativeAccount,
    )?;
    let (mut peer, child) = launch(executable, prepared).await?;
    let mut owned = OwnedWorker { child, coordinator };
    let timeout = Duration::from_secs(60);
    let mut terminal = None;
    let mut stopped_end=false;
    let mut upstream_error=None;
    let mut drain_deadline=None;
    let observe_attributes=attributes.metadata.is_some();
    let result = async {
        if *cancel.borrow() { return Err(io::Error::new(io::ErrorKind::Interrupted, "transfer cancelled before execution")); }
        peer.control(Control::Transfer { request: Request { operation: operation.clone(), identity, native_path, attributes } }).await?;
        let run = async {
            loop {
                let remaining=drain_deadline.map_or(timeout,|deadline:tokio::time::Instant|deadline.saturating_duration_since(tokio::time::Instant::now()).min(timeout));
                let frame = tokio::time::timeout(remaining, peer.receive()).await.map_err(io::Error::other)??;
                if frame.tag == 8 {
                    if !matches!(operation, Operation::Download { .. }) || frame.bytes.is_empty() || terminal.is_some() || stopped_end { return Err(io::Error::other("unexpected transfer output chunk")); }
                    let accepted=match parent_io_or_stop(&records,stream.send_binary_frame(&frame.bytes,timeout)).await {
                        Ok(result)=>result.is_some(),
                        Err(error)=>{upstream_error.get_or_insert(error);drain_deadline.get_or_insert(tokio::time::Instant::now()+Duration::from_secs(10));false},
                    };
                    peer.control(Control::TransferAck { accepted }).await?;
                    continue;
                }
                let Control::TransferReply { reply } = decode(frame)? else { return Err(io::Error::other("unexpected transfer worker reply")); };
                if stopped_end && !matches!(reply,Reply::Done {..}|Reply::Unlock {..}) {return Err(io::Error::other("transfer I/O continued after stop acknowledgement"));}
                match reply {
                    Reply::End=>{
                        if terminal.is_some() || stopped_end {return Err(io::Error::other("duplicate transfer end"));}
                        stopped_end=true;
                        peer.control(Control::TransferAck {accepted:true}).await?;
                    },
                    Reply::Lock { id, key } => {
                        let accepted = owned.coordinator.lock(id, key)?;
                        peer.control(Control::TransferAck { accepted }).await?;
                    },
                    Reply::Unlock { id } => owned.coordinator.unlock(id)?,
                    Reply::Record { event } => {
                        // The parent still binds every fact to the original
                        // accepted operation. Reconciliation can record only
                        // observed publication/attributes, not content progress
                        // or a new publication intent.
                        if matches!(operation, Operation::Reconcile { .. }) && (!observe_attributes || !matches!(event,TransferRecord::Published|TransferRecord::Metadata {..})) { return Err(io::Error::other("reconciliation cannot alter transfer progress or intent")); }
                        if matches!(event,TransferRecord::StopCompleted){return Err(io::Error::other("only the parent can confirm worker cleanup"));}
                        let polling=matches!(event,TransferRecord::CheckStop|TransferRecord::CancelRequested);
                        let result=records.record(event).await;
                        if !polling {result.as_ref().map_err(|error|io::Error::other(error.to_string()))?;}
                        peer.control(Control::TransferAck { accepted:result.is_ok() }).await?;
                    },
                    Reply::ReadChunk => {
                        if !matches!(operation, Operation::Upload { .. }) || terminal.is_some() { return Err(io::Error::other("unexpected transfer input request")); }
                        match parent_io_or_stop(&records,stream.receive_binary_frame(timeout)).await {
                            Ok(Some(bytes))=>{if bytes.is_empty() || bytes.len()>BINARY_LIMIT{return Err(io::Error::other("invalid upload input chunk"));}peer.send(7,&bytes).await?;},
                            Ok(None)=>peer.send(9,&[]).await?,
                            Err(error)=>{
                                upstream_error.get_or_insert(error);drain_deadline.get_or_insert(tokio::time::Instant::now()+Duration::from_secs(10));
                                // Let the receiver sync its completed frames and
                                // send its final checkpoint over the owned IPC.
                                peer.send(9,&[]).await?;
                            },
                        }
                    },
                    Reply::Response { value, end } => {
                        if terminal.is_some() || !matches!(&*value, DeviceTaskResponse::FileReady { .. } | DeviceTaskResponse::FileProgress { .. } | DeviceTaskResponse::FileComplete { .. } | DeviceTaskResponse::Error { .. }) { return Err(io::Error::other("invalid transfer response")); }
                        if end { terminal = Some(*value); }
                        else if let Err(error)=stream.response(&value, false, timeout).await {
                            upstream_error.get_or_insert_with(||io::Error::other(error));drain_deadline.get_or_insert(tokio::time::Instant::now()+Duration::from_secs(10));
                            peer.control(Control::TransferAck {accepted:false}).await?;continue;
                        }
                        peer.control(Control::TransferAck { accepted: true }).await?;
                    },
                    Reply::Done { error } => {
                        if let Some(original)=upstream_error.take(){return Err(io::Error::new(original.kind(),match error {Some(worker)=>format!("{original}; worker: {worker}"),None=>original.to_string()}));}
                        return error.map_or(Ok(()), |error| Err(io::Error::other(error)));
                    },
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
    if stopped_end && result.is_ok() {
        records.record(TransferRecord::StopCompleted).await?;
        stream.end(timeout).await.map_err(io::Error::other)?;
    } else if let Some(value) = terminal {
        stream
            .response(&value, true, timeout)
            .await
            .map_err(io::Error::other)?;
    } else if result.is_ok() {
        return Err(io::Error::other("transfer finished without receipt"));
    }
    result
}

async fn parent_io_or_stop<T>(records:&LocalRecorder,io:impl std::future::Future<Output=Result<T,TaskServiceError>>)->io::Result<Option<T>> {
    tokio::pin!(io);
    let mut tick=tokio::time::interval_at(tokio::time::Instant::now()+Duration::from_millis(250),Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {tokio::select! {biased;
        _=tick.tick()=>if records.record(TransferRecord::CheckStop).await.is_err(){return Ok(None)},
        result=&mut io=>return result.map(Some).map_err(io::Error::other),
    }}
}
