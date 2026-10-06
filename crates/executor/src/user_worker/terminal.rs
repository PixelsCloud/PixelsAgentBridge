//! A fixed-identity PTY in the existing user worker. Requests are serialized by
//! an actor so a cancelled network read cannot leave a half-consumed IPC frame.
use super::*;
use pab_terminal::{MAX_INPUT_BYTES, MAX_READ_BYTES, TerminalOutput, TerminalSession};
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
                // Close/drop (including ConPTY handles) before acknowledging.
                tokio::task::spawn_blocking(move || {
                    session.close()?;
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
                reply: Reply::Opened { .. }
            }
        ) {
            return Err(io::Error::other("terminal open not confirmed"));
        }
        let (calls, mut receive) = mpsc::channel::<Call>(8);
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
