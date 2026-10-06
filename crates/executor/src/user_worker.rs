//! Internal mode of pab-executor. No machine config, database or network login.
use crate::task_service::{
    TaskServiceError,
    command::{self, CommandMessage, CommandSink},
};
use pab_os_control::execution::{PreparedUser, UserIdentity, channel, current_identity};
use pab_protocol::{CommandTaskSpec, OutputStream, TaskEventKind};
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, io, path::Path, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, watch},
};

const HANDSHAKE: Duration = Duration::from_secs(15);
const CONTROL_LIMIT: usize = 64 * 1024;
const BINARY_LIMIT: usize = 256 * 1024;
pub(crate) mod git;
pub(crate) mod terminal;
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Control {
    Git {
        request: git::Request,
    },
    GitReply {
        reply: git::Reply,
    },
    GitAck {
        error: Option<String>,
    },
    Hello {
        version: u16,
        identity: UserIdentity,
    },
    Command {
        command: CommandTaskSpec,
    },
    Terminal {
        request: terminal::Request,
    },
    TerminalReply {
        reply: terminal::Reply,
    },
    Event {
        event: TaskEventKind,
    },
    Complete {
        stream: OutputStream,
    },
    Cancel {
        reason: String,
    },
    Done {
        error: Option<String>,
    },
}
struct Frame {
    tag: u8,
    bytes: Vec<u8>,
}
struct Peer {
    writer: tokio::io::WriteHalf<channel::WorkerStream>,
    incoming: mpsc::Receiver<io::Result<Frame>>,
    reader: tokio::task::JoinHandle<()>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
impl Peer {
    fn new(stream: channel::WorkerStream) -> Self {
        let (mut reader, writer) = tokio::io::split(stream);
        let (send, incoming) = mpsc::channel(8);
        let reader = tokio::spawn(async move {
            loop {
                let value = read_frame(&mut reader).await;
                let failed = value.is_err();
                if send.send(value).await.is_err() || failed {
                    break;
                }
            }
        });
        Self {
            writer,
            incoming,
            reader,
        }
    }
    async fn receive(&mut self) -> io::Result<Frame> {
        self.incoming
            .recv()
            .await
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "worker channel closed"))?
    }
    async fn control(&mut self, value: Control) -> io::Result<()> {
        self.send(0, &serde_json::to_vec(&value)?).await
    }
    async fn send(&mut self, tag: u8, bytes: &[u8]) -> io::Result<()> {
        tokio::time::timeout(
            Duration::from_secs(30),
            write_frame(&mut self.writer, tag, bytes),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "worker write timed out"))?
    }
}
fn decode(frame: Frame) -> io::Result<Control> {
    if frame.tag != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected worker control frame",
        ));
    }
    serde_json::from_slice(&frame.bytes).map_err(io::Error::other)
}
async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Frame> {
    let tag = reader.read_u8().await?;
    let len = reader.read_u32().await? as usize;
    let limit = match tag {
        0 => CONTROL_LIMIT,
        1..=4 => BINARY_LIMIT,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown worker frame",
            ));
        }
    };
    if len > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "worker frame too large",
        ));
    }
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes).await?;
    Ok(Frame { tag, bytes })
}
async fn write_frame(
    writer: &mut (impl AsyncWrite + Unpin),
    tag: u8,
    bytes: &[u8],
) -> io::Result<()> {
    let limit = match tag {
        0 => CONTROL_LIMIT,
        1..=4 => BINARY_LIMIT,
        _ => 0,
    };
    if limit == 0 || bytes.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid worker output frame",
        ));
    }
    writer.write_u8(tag).await?;
    writer.write_u32(bytes.len() as u32).await?;
    writer.write_all(bytes).await
}
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);
impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Called before machine logging/config initialization by the internal CLI mode.
pub async fn run(address: &str, parent_pid: u32) -> io::Result<()> {
    let mut peer = Peer::new(channel::connect(address, parent_pid, HANDSHAKE).await?);
    peer.control(Control::Hello {
        version: 1,
        identity: current_identity()?,
    })
    .await?;
    let request = tokio::time::timeout(HANDSHAKE, peer.receive())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "worker request timed out"))??;
    let command = match decode(request)? {
        Control::Git { request } => return git::serve(peer, request).await,
        Control::Command { command } => command,
        Control::Terminal {
            request: terminal::Request::Open { cols, rows },
        } => {
            return terminal::serve(peer, cols, rows).await;
        }
        _ => return Err(io::Error::other("expected worker operation")),
    };
    crate::task_service::validate_command(&command).map_err(io::Error::other)?;
    let (cancel, receiver) = watch::channel(None);
    let (output, mut incoming) = mpsc::channel(16);
    let mut worker = AbortOnDrop(tokio::spawn(command::execute(
        CommandSink::Worker(output),
        command,
        receiver,
    )));
    loop {
        tokio::select! {
            value=peer.receive()=>{
                match decode(value?)? {
                    Control::Cancel{reason} if !reason.is_empty() && reason.len()<=1024=>{let _=cancel.send(Some(reason));}
                    _=>return Err(io::Error::other("unexpected worker input")),
                }
            }
            value=incoming.recv()=>match value {
                Some(CommandMessage::Event(event))=>peer.control(Control::Event{event}).await?,
                Some(CommandMessage::Complete(stream))=>peer.control(Control::Complete{stream}).await?,
                Some(CommandMessage::Output(stream,bytes))=>peer.send(if stream==OutputStream::Stdout {1}else{2},&bytes).await?,
                None=>{
                    let error=match (&mut worker.0).await {Ok(Ok(()))=>None,Ok(Err(e))=>Some(e.to_string()),Err(e)=>Some(e.to_string())};
                    peer.control(Control::Done{error}).await?;
                    return Ok(());
                }
            }
        }
    }
}

pub(crate) async fn execute(
    executable: &Path,
    prepared: PreparedUser,
    command: CommandTaskSpec,
    sink: CommandSink,
    cancel: watch::Receiver<Option<String>>,
) -> Result<(), TaskServiceError> {
    let (mut peer, mut child) = launch(executable, prepared).await?;
    peer.control(Control::Command { command }).await?;
    execute_started(&mut peer, &mut child, sink, cancel).await
}

async fn launch(
    executable: &Path,
    prepared: PreparedUser,
) -> io::Result<(Peer, pab_os_control::execution::UserProcess)> {
    let expected = prepared.identity().clone();
    let mut listener = channel::WorkerListener::bind(&expected)?;
    let args = vec![
        OsString::from("--user-worker"),
        OsString::from(listener.address()),
        OsString::from(std::process::id().to_string()),
    ];
    let executable = executable.to_owned();
    let cwd = expected.home.clone();
    let child =
        tokio::task::spawn_blocking(move || prepared.spawn(&executable, &args, &cwd)).await??;
    let mut peer = Peer::new(listener.accept(child.id(), HANDSHAKE).await?);
    let hello = tokio::time::timeout(HANDSHAKE, peer.receive())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "worker identity timed out"))??;
    match decode(hello)? {
        Control::Hello {
            version: 1,
            identity,
        } if identity == expected => {}
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "native worker identity mismatch",
            ));
        }
    }
    Ok((peer, child))
}

async fn execute_started(
    peer: &mut Peer,
    child: &mut pab_os_control::execution::UserProcess,
    sink: CommandSink,
    mut cancel: watch::Receiver<Option<String>>,
) -> Result<(), TaskServiceError> {
    let initial = cancel.borrow().clone();
    let mut cancel_deadline = None;
    if let Some(reason) = initial {
        peer.control(Control::Cancel { reason }).await?;
        cancel_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(10));
    }
    let mut terminal = None;
    let mut cancellation_open = true;
    loop {
        let wait_cancel = async {
            match cancel_deadline {
                Some(end) => tokio::time::sleep_until(end).await,
                None => std::future::pending::<()>().await,
            }
        };
        let frame = tokio::select! {
            value=peer.receive()=>value?,
            changed=cancel.changed(),if cancellation_open && cancel_deadline.is_none()=>{
                if changed.is_err(){cancellation_open=false;}else {
                    let reason=cancel.borrow().clone();
                    if let Some(reason)=reason {
                        peer.control(Control::Cancel{reason}).await?;
                        cancel_deadline=Some(tokio::time::Instant::now()+Duration::from_secs(10));
                    }
                }
                continue;
            },
            _=wait_cancel=>return Err(io::Error::new(io::ErrorKind::TimedOut,"user worker cancellation not confirmed").into()),
        };
        match frame.tag {
            1 | 2 => {
                sink.send(CommandMessage::Output(
                    if frame.tag == 1 {
                        OutputStream::Stdout
                    } else {
                        OutputStream::Stderr
                    },
                    frame.bytes,
                ))
                .await?
            }
            _ => match decode(frame)? {
                Control::Event { event } => {
                    if matches!(
                        event,
                        TaskEventKind::Succeeded { .. }
                            | TaskEventKind::Failed { .. }
                            | TaskEventKind::Cancelled { .. }
                            | TaskEventKind::Interrupted { .. }
                    ) {
                        if terminal.replace(event).is_some() {
                            return Err(io::Error::other("duplicate worker terminal event").into());
                        }
                    } else {
                        sink.event(event).await?;
                    }
                }
                Control::Complete { stream } => sink.send(CommandMessage::Complete(stream)).await?,
                Control::Done { error } => {
                    if let Some(error) = error {
                        return Err(io::Error::other(error).into());
                    }
                    break;
                }
                _ => return Err(io::Error::other("unexpected worker response").into()),
            },
        }
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(code) = child.try_wait()? {
            if code != 0 {
                return Err(io::Error::other(format!("user worker exited {code}")).into());
            }
            break;
        }
        if tokio::time::Instant::now() > deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "user worker did not exit").into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    child.terminate()?; // Terminate any remaining Windows job members before terminal result.
    sink.event(terminal.ok_or_else(|| io::Error::other("worker omitted terminal result"))?)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn binary_frames_round_trip_and_reject_oversized_or_partial_messages() {
        let bytes = (0..BINARY_LIMIT)
            .map(|n| (n % 251) as u8)
            .collect::<Vec<_>>();
        let mut encoded = vec![];
        write_frame(&mut encoded, 1, &bytes).await.unwrap();
        let frame = read_frame(&mut encoded.as_slice()).await.unwrap();
        assert_eq!(frame.tag, 1);
        assert_eq!(frame.bytes, bytes);
        for invalid in [
            vec![9, 0, 0, 0, 0],
            vec![0, 0, 1, 0, 1],
            vec![1, 0, 0, 0, 2, 1],
        ] {
            assert!(read_frame(&mut invalid.as_slice()).await.is_err());
        }
    }
}
