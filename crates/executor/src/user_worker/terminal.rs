//! A fixed-identity PTY in the existing user worker. Requests are serialized by
//! an actor so a cancelled network read cannot leave a half-consumed IPC frame.
use super::*;
use pab_terminal::{
    MAX_INPUT_BYTES, MAX_READ_BYTES, MAX_RETAINED_BYTES, TerminalOutput, TerminalSession,
};
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Request {
    Open { cols: u16, rows: u16 },
    Input { size: usize },
    Read { offset: u64, limit: usize },
    Resize { cols: u16, rows: u16 },
    Close,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Reply {
    Opened {
        shell: String,
        startup: pab_protocol::TerminalStartup,
    },
    Applied,
    Output {
        retained_from: u64,
        offset: u64,
        next_offset: u64,
        size: usize,
        ended: bool,
    },
    Closed,
}

pub(crate) fn shell() -> (&'static str, &'static [&'static str]) {
    if cfg!(windows) {
        ("powershell.exe", &["-NoLogo", "-NoProfile"])
    } else if cfg!(target_os = "macos") {
        ("/bin/zsh", &["-l", "-i"])
    } else {
        ("/bin/sh", &["-i"])
    }
}

pub(crate) fn startup() -> pab_protocol::TerminalStartup {
    use pab_protocol::{TerminalStartup, TerminalStartupMode};
    TerminalStartup {
        arguments: shell().1.iter().map(|arg| (*arg).to_owned()).collect(),
        mode: if cfg!(windows) {
            TerminalStartupMode::InteractiveNoProfile
        } else if cfg!(target_os = "macos") {
            TerminalStartupMode::InteractiveLogin
        } else {
            TerminalStartupMode::Interactive
        },
    }
}

pub(super) async fn serve(mut peer: Peer, cols: u16, rows: u16) -> io::Result<()> {
    let (shell, args) = shell();
    let session = Arc::new(
        tokio::task::spawn_blocking(move || TerminalSession::start(shell, args, cols, rows))
            .await
            .map_err(io::Error::other)?
            .map_err(io::Error::other)?,
    );
    peer.control(Control::TerminalReply {
        reply: Reply::Opened {
            shell: shell.into(),
            startup: startup(),
        },
    })
    .await?;
    // On any protocol/EOF error, dropping the last session closes the PTY before
    // main tears down the worker's own group (PTY has a separate Unix session).
    loop {
        let Control::Terminal { request } = decode(peer.receive().await?)? else {
            return Err(io::Error::other("expected terminal request"));
        };
        let reply = match request {
            Request::Input { size } if size > 0 && size <= MAX_INPUT_BYTES => {
                let frame = tokio::time::timeout(Duration::from_secs(30), peer.receive())
                    .await
                    .map_err(io::Error::other)??;
                if frame.tag != 3 || frame.bytes.len() != size {
                    return Err(io::Error::other("invalid terminal input frame"));
                }
                let session = Arc::clone(&session);
                tokio::task::spawn_blocking(move || session.input(&frame.bytes))
                    .await
                    .map_err(io::Error::other)?
                    .map_err(io::Error::other)?;
                Reply::Applied
            }
            Request::Read { offset, limit } if limit > 0 && limit <= MAX_READ_BYTES => {
                let output = session.read(offset, limit);
                peer.control(Control::TerminalReply {
                    reply: Reply::Output {
                        retained_from: output.retained_from,
                        offset: output.offset,
                        next_offset: output.next_offset,
                        size: output.bytes.len(),
                        ended: output.ended,
                    },
                })
                .await?;
                if !output.bytes.is_empty() {
                    peer.send(4, &output.bytes).await?;
                }
                continue;
            }
            Request::Resize { cols, rows } => {
                let session = Arc::clone(&session);
                tokio::task::spawn_blocking(move || session.resize(cols, rows))
                    .await
                    .map_err(io::Error::other)?
                    .map_err(io::Error::other)?;
                Reply::Applied
            }
            Request::Close => {
                // Preserve the bounded tail before dropping the PTY/worker. The
                // network client drains it after the close acknowledgement.
                let closing = Arc::clone(&session);
                tokio::task::spawn_blocking(move || closing.close())
                    .await
                    .map_err(io::Error::other)?
                    .map_err(io::Error::other)?;
                let until = tokio::time::Instant::now() + Duration::from_secs(3);
                let mut offset = 0;
                loop {
                    let output = session.read(offset, MAX_READ_BYTES);
                    peer.control(Control::TerminalReply {
                        reply: Reply::Output {
                            retained_from: output.retained_from,
                            offset: output.offset,
                            next_offset: output.next_offset,
                            size: output.bytes.len(),
                            ended: output.ended,
                        },
                    })
                    .await?;
                    if !output.bytes.is_empty() {
                        peer.send(4, &output.bytes).await?;
                    }
                    offset = output.next_offset;
                    if output.ended {
                        break;
                    }
                    if tokio::time::Instant::now() >= until {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "terminal tail did not finish",
                        ));
                    }
                    if output.bytes.is_empty() {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                }
                tokio::task::spawn_blocking(move || {
                    drop(session);
                    Ok::<_, pab_terminal::TerminalError>(())
                })
                .await
                .map_err(io::Error::other)?
                .map_err(io::Error::other)?;
                peer.control(Control::TerminalReply {
                    reply: Reply::Closed,
                })
                .await?;
                return Ok(());
            }
            _ => return Err(io::Error::other("invalid terminal request")),
        };
        peer.control(Control::TerminalReply { reply }).await?;
    }
}

enum Action {
    Input(Vec<u8>),
    Read(u64, usize),
    Resize(u16, u16),
    Close,
}
enum ResultValue {
    Applied,
    Output(TerminalOutput),
}
struct Call {
    action: Action,
    answer: oneshot::Sender<io::Result<ResultValue>>,
}
pub(crate) struct UserTerminal {
    calls: mpsc::Sender<Call>,
    closed: tokio::sync::Mutex<bool>,
    tail: Arc<tokio::sync::Mutex<Option<TerminalOutput>>>,
}
impl UserTerminal {
    pub(crate) async fn start(
        executable: &Path,
        user: PreparedUser,
        cols: u16,
        rows: u16,
    ) -> io::Result<Self> {
        let (mut peer, mut child) = launch(executable, user).await?;
        peer.control(Control::Terminal {
            request: Request::Open { cols, rows },
        })
        .await?;
        let opened = tokio::time::timeout(HANDSHAKE, peer.receive())
            .await
            .map_err(io::Error::other)??;
        if !matches!(
            decode(opened)?,
            Control::TerminalReply {
                reply: Reply::Opened { shell: actual_shell, startup: actual_startup }
            } if actual_shell == shell().0 && actual_startup == startup()
        ) {
            return Err(io::Error::other("terminal open not confirmed"));
        }
        let (calls, mut receive) = mpsc::channel::<Call>(8);
        let tail = Arc::new(tokio::sync::Mutex::new(None));
        let final_tail = Arc::clone(&tail);
        tokio::spawn(async move {
            while let Some(call) = receive.recv().await {
                let closing = matches!(call.action, Action::Close);
                let result =
                    tokio::time::timeout(Duration::from_secs(30), exchange(&mut peer, call.action))
                        .await
                        .map_err(io::Error::other)
                        .and_then(|r| r);
                let failed = result.is_err();
                if closing && !failed {
                    let cleanup = wait_exit(&mut child).await;
                    if cleanup.is_ok() {
                        if let Ok(ResultValue::Output(output)) = result {
                            *final_tail.lock().await = Some(output);
                        }
                    }
                    let _ = call.answer.send(cleanup.map(|_| ResultValue::Applied));
                    return;
                }
                let _ = call.answer.send(result);
                if failed {
                    break;
                }
            }
            // Last owner dropped or an operation is unconfirmed. EOF gives the
            // worker an opportunity to close its PTY before forced containment.
            drop(peer);
            let _ = wait_exit(&mut child).await;
        });
        Ok(Self {
            calls,
            closed: tokio::sync::Mutex::new(false),
            tail,
        })
    }
    async fn call(&self, action: Action) -> io::Result<ResultValue> {
        let (answer, result) = oneshot::channel();
        self.calls
            .send(Call { action, answer })
            .await
            .map_err(|_| io::Error::other("user terminal ended"))?;
        result
            .await
            .map_err(|_| io::Error::other("user terminal result unconfirmed"))?
    }
    pub(crate) async fn input(&self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() || bytes.len() > MAX_INPUT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid terminal input",
            ));
        }
        self.call(Action::Input(bytes.to_vec())).await.map(|_| ())
    }
    pub(crate) async fn read(&self, offset: u64, limit: usize) -> io::Result<TerminalOutput> {
        if limit == 0 || limit > MAX_READ_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid terminal read limit",
            ));
        }
        if let Some(tail) = self.tail.lock().await.as_ref() {
            let offset = offset.max(tail.retained_from);
            let start = (offset - tail.retained_from).min(tail.bytes.len() as u64) as usize;
            let bytes = tail
                .bytes
                .iter()
                .skip(start)
                .take(limit)
                .copied()
                .collect::<Vec<_>>();
            let next_offset = offset + bytes.len() as u64;
            return Ok(TerminalOutput {
                retained_from: tail.retained_from,
                offset,
                next_offset,
                ended: next_offset >= tail.next_offset,
                bytes,
            });
        }
        match self.call(Action::Read(offset, limit)).await? {
            ResultValue::Output(output) => Ok(output),
            _ => Err(io::Error::other("missing terminal output")),
        }
    }
    pub(crate) async fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.call(Action::Resize(cols, rows)).await.map(|_| ())
    }
    pub(crate) async fn close(&self) -> io::Result<()> {
        let mut closed = self.closed.lock().await;
        if !*closed {
            self.call(Action::Close).await?;
            *closed = true;
        }
        Ok(())
    }
}
async fn wait_exit(child: &mut pab_os_control::execution::UserProcess) -> io::Result<()> {
    let until = tokio::time::Instant::now() + Duration::from_secs(5);
    while child.try_wait()?.is_none() {
        if tokio::time::Instant::now() >= until {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "terminal worker cleanup not confirmed",
            ));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    child.terminate()
}
async fn exchange(peer: &mut Peer, action: Action) -> io::Result<ResultValue> {
    let request = match &action {
        Action::Input(bytes) => Request::Input { size: bytes.len() },
        Action::Read(offset, limit) => Request::Read {
            offset: *offset,
            limit: *limit,
        },
        Action::Resize(cols, rows) => Request::Resize {
            cols: *cols,
            rows: *rows,
        },
        Action::Close => Request::Close,
    };
    peer.control(Control::Terminal { request }).await?;
    if let Action::Input(bytes) = &action {
        peer.send(3, bytes).await?;
    }
    if matches!(action, Action::Close) {
        let mut tail = TerminalOutput {
            retained_from: 0,
            offset: 0,
            next_offset: 0,
            bytes: vec![],
            ended: false,
        };
        loop {
            match decode(peer.receive().await?)? {
                Control::TerminalReply {
                    reply:
                        Reply::Output {
                            retained_from,
                            offset,
                            next_offset,
                            size,
                            ended,
                        },
                } if !tail.ended
                    && size <= MAX_READ_BYTES
                    && offset == tail.next_offset.max(retained_from)
                    && offset.checked_add(size as u64) == Some(next_offset) =>
                {
                    if retained_from > tail.retained_from {
                        let discard = (retained_from - tail.retained_from)
                            .min(tail.bytes.len() as u64)
                            as usize;
                        tail.bytes.drain(..discard);
                        tail.retained_from = retained_from;
                        tail.offset = retained_from;
                    }
                    if size > 0 {
                        let frame = peer.receive().await?;
                        if frame.tag != 4
                            || frame.bytes.len() != size
                            || tail.bytes.len() + size > MAX_RETAINED_BYTES
                        {
                            return Err(io::Error::other("invalid terminal closing output"));
                        }
                        tail.bytes.extend_from_slice(&frame.bytes);
                    }
                    tail.next_offset = next_offset;
                    tail.ended = ended;
                }
                Control::TerminalReply {
                    reply: Reply::Closed,
                } if tail.ended => return Ok(ResultValue::Output(tail)),
                _ => return Err(io::Error::other("terminal closing output not confirmed")),
            }
        }
    }
    let Control::TerminalReply { reply } = decode(peer.receive().await?)? else {
        return Err(io::Error::other("missing terminal reply"));
    };
    match (action, reply) {
        (Action::Input(_) | Action::Resize(..), Reply::Applied)
        | (Action::Close, Reply::Closed) => Ok(ResultValue::Applied),
        (
            Action::Read(requested, limit),
            Reply::Output {
                retained_from,
                offset,
                next_offset,
                size,
                ended,
            },
        ) if offset == requested.max(retained_from)
            && size <= limit
            && offset.checked_add(size as u64) == Some(next_offset) =>
        {
            let bytes = if size == 0 {
                vec![]
            } else {
                let frame = peer.receive().await?;
                if frame.tag != 4 || frame.bytes.len() != size {
                    return Err(io::Error::other("invalid terminal output frame"));
                }
                frame.bytes
            };
            Ok(ResultValue::Output(TerminalOutput {
                retained_from,
                offset,
                next_offset,
                bytes,
                ended,
            }))
        }
        _ => Err(io::Error::other("unexpected terminal reply")),
    }
}
