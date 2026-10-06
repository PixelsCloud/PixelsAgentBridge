//! Recoverable process boundary for accessibility providers. Native objects never
//! cross this boundary; a killed worker invalidates its whole reference registry.
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, SyncSender},
    thread::{self, JoinHandle},
    time::Duration,
};

const MAX_FRAME: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerFailure {
    Unavailable,
    InvalidFrame,
    Deadline,
    Stopped,
}

struct Exchange {
    request: Vec<u8>,
    reply: SyncSender<Result<Value, WorkerFailure>>,
}

pub struct WorkerProcess {
    child: Child,
    sender: Option<SyncSender<Exchange>>,
    io: Option<JoinHandle<()>>,
    stopped: bool,
}

impl WorkerProcess {
    pub fn process_id(&self) -> u32 {
        self.child.id()
    }
    pub fn spawn(command: &mut Command) -> Result<Self, WorkerFailure> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn().map_err(|_| WorkerFailure::Unavailable)?;
        let mut input = child.stdin.take().expect("piped worker stdin");
        let output = child.stdout.take().expect("piped worker stdout");
        let (sender, receiver): (SyncSender<Exchange>, Receiver<Exchange>) = mpsc::sync_channel(1);
        let io = match thread::Builder::new()
            .name("pab-ui-pipe".into())
            .spawn(move || {
                let mut output = BufReader::new(output);
                while let Ok(exchange) = receiver.recv() {
                    let result = (|| {
                        input
                            .write_all(&exchange.request)
                            .map_err(|_| WorkerFailure::Stopped)?;
                        input.write_all(b"\n").map_err(|_| WorkerFailure::Stopped)?;
                        input.flush().map_err(|_| WorkerFailure::Stopped)?;
                        read_frame(&mut output)
                    })();
                    let failed = result.is_err();
                    let _ = exchange.reply.send(result);
                    if failed {
                        break;
                    }
                }
            }) {
            Ok(io) => io,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WorkerFailure::Unavailable);
            }
        };
        Ok(Self {
            child,
            sender: Some(sender),
            io: Some(io),
            stopped: false,
        })
    }

    /// Sole caller holds &mut self; no unbounded queue or replacement threads.
    /// Any error after sending may have side effects: caller must not replay.
    pub fn exchange(&mut self, request: &Value, timeout: Duration) -> Result<Value, WorkerFailure> {
        if self.stopped {
            return Err(WorkerFailure::Stopped);
        }
        let request = serde_json::to_vec(request).map_err(|_| WorkerFailure::InvalidFrame)?;
        if request.len() > MAX_FRAME {
            return Err(WorkerFailure::InvalidFrame);
        }
        let (reply, receiver) = mpsc::sync_channel(1);
        if self
            .sender
            .as_ref()
            .ok_or(WorkerFailure::Stopped)?
            .try_send(Exchange { request, reply })
            .is_err()
        {
            self.stop();
            return Err(WorkerFailure::Stopped);
        }
        let result = match receiver.recv_timeout(timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(WorkerFailure::Deadline),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(WorkerFailure::Stopped),
        };
        if result.is_err() {
            self.stop();
        }
        result
    }

    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        // Close the queue before joining, and kill before waiting for the pipe.
        // Worker mode must not launch descendants inheriting its stdio handles.
        self.sender.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(io) = self.io.take() {
            let _ = io.join();
        }
    }
}
impl Drop for WorkerProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn read_frame(reader: &mut impl BufRead) -> Result<Value, WorkerFailure> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_FRAME + 2) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| WorkerFailure::Stopped)?;
    if bytes.last() != Some(&b'\n') || bytes.len() > MAX_FRAME + 1 {
        return Err(WorkerFailure::InvalidFrame);
    }
    serde_json::from_slice(&bytes).map_err(|_| WorkerFailure::InvalidFrame)
}

pub fn write_frame(writer: &mut impl Write, value: &Value) -> Result<(), WorkerFailure> {
    let bytes = serde_json::to_vec(value).map_err(|_| WorkerFailure::InvalidFrame)?;
    if bytes.len() > MAX_FRAME {
        return Err(WorkerFailure::InvalidFrame);
    }
    writer
        .write_all(&bytes)
        .and_then(|_| writer.write_all(b"\n"))
        .and_then(|_| writer.flush())
        .map_err(|_| WorkerFailure::Stopped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{io::Cursor, time::Instant};

    #[test]
    fn frames_reject_eof_truncation_invalid_json_and_oversize() {
        for input in [
            b"".to_vec(),
            b"{}".to_vec(),
            b"broken\n".to_vec(),
            vec![b'x'; MAX_FRAME + 2],
        ] {
            assert!(read_frame(&mut Cursor::new(input)).is_err());
        }
        let mut data = Cursor::new(b"{\"id\":1}\n{\"id\":2}\n");
        assert_eq!(read_frame(&mut data).unwrap(), json!({"id":1}));
        assert_eq!(read_frame(&mut data).unwrap(), json!({"id":2}));
        assert!(write_frame(&mut Vec::new(), &json!("x".repeat(MAX_FRAME))).is_err());
    }

    #[cfg(windows)]
    fn fixture(mode: &str) -> Command {
        let mut c = Command::new("powershell.exe");
        let script = match mode {
            "echo" => {
                "while ($null -ne ($line = [Console]::ReadLine())) { [Console]::WriteLine($line) }"
            }
            "hang" => "Start-Sleep -Seconds 60",
            _ => "[Console]::WriteLine('invalid')",
        };
        c.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ]);
        c
    }
    #[cfg(unix)]
    fn fixture(mode: &str) -> Command {
        let mut c = Command::new("/bin/sh");
        let script = match mode {
            "echo" => "while IFS= read -r line; do printf '%s\\n' \"$line\"; done",
            // exec is critical: there must be no descendant retaining the pipes.
            "hang" => "exec sleep 60",
            _ => "printf 'invalid\\n'",
        };
        c.args(["-c", script]);
        c
    }
    #[cfg(any(windows, unix))]
    #[test]
    fn hung_worker_is_killed_reaped_and_can_be_replaced() {
        let mut worker = WorkerProcess::spawn(&mut fixture("hang")).unwrap();
        let start = Instant::now();
        assert_eq!(
            worker.exchange(&json!({"id":1}), Duration::from_millis(200)),
            Err(WorkerFailure::Deadline)
        );
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(worker.child.try_wait().unwrap().is_some());
        assert_eq!(
            worker.exchange(&json!({"id":2}), Duration::from_secs(1)),
            Err(WorkerFailure::Stopped)
        );
        let mut replacement = WorkerProcess::spawn(&mut fixture("echo")).unwrap();
        for id in 0..3 {
            assert_eq!(
                replacement
                    .exchange(&json!({"id":id}), Duration::from_secs(5))
                    .unwrap(),
                json!({"id":id})
            );
        }
        replacement.stop();
        assert!(replacement.child.try_wait().unwrap().is_some());
    }
    #[cfg(any(windows, unix))]
    #[test]
    fn invalid_worker_output_also_reclaims_process_and_io_thread() {
        let mut worker = WorkerProcess::spawn(&mut fixture("invalid")).unwrap();
        assert!(worker.exchange(&json!({}), Duration::from_secs(5)).is_err());
        assert!(worker.child.try_wait().unwrap().is_some());
        assert!(worker.io.is_none());
    }
}
