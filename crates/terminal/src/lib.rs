use std::{
    collections::VecDeque,
    io::{Read, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use thiserror::Error;

const MAX_RETAINED_BYTES: usize = 1024 * 1024;
pub const MAX_INPUT_BYTES: usize = 4 * 1024;
pub const MAX_READ_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone)]
pub struct TerminalOutput {
    pub retained_from: u64,
    pub offset: u64,
    pub next_offset: u64,
    pub bytes: Vec<u8>,
    pub ended: bool,
}

#[derive(Default)]
struct OutputBuffer {
    retained_from: u64,
    data: VecDeque<u8>,
    ended: bool,
    last_append_at: Option<Instant>,
}

impl OutputBuffer {
    fn append(&mut self, bytes: &[u8]) {
        self.last_append_at = Some(Instant::now());
        self.data.extend(bytes);
        while self.data.len() > MAX_RETAINED_BYTES {
            self.data.pop_front();
            self.retained_from += 1;
        }
    }

    fn read(&self, offset: u64, max_bytes: usize) -> TerminalOutput {
        let offset = offset.max(self.retained_from);
        let start = (offset - self.retained_from) as usize;
        let bytes = self
            .data
            .iter()
            .skip(start)
            .take(max_bytes.min(MAX_READ_BYTES))
            .copied()
            .collect::<Vec<_>>();
        let next_offset = offset + bytes.len() as u64;
        TerminalOutput {
            retained_from: self.retained_from,
            offset,
            next_offset,
            bytes,
            ended: self.ended && next_offset >= self.retained_from + self.data.len() as u64,
        }
    }
}

pub struct TerminalSession {
    // Drop the input pipe before the ConPTY handle so Windows can finish closing it.
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn portable_pty::Child + Send>>,
    output: Arc<Mutex<OutputBuffer>>,
    exit_seen_at: Mutex<Option<Instant>>,
}

impl TerminalSession {
    pub fn start(shell: &str, args: &[&str], cols: u16, rows: u16) -> Result<Self, TerminalError> {
        validate_size(cols, rows)?;
        let pair = NativePtySystem::default()
            .openpty(PtySize {
                cols,
                rows,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TerminalError::Pty(error.to_string()))?;
        let mut command = CommandBuilder::new(shell);
        command.args(args);
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| TerminalError::Pty(error.to_string()))?;
        drop(pair.slave);
        let mut reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => {
                let _ = child.kill();
                return Err(TerminalError::Pty(error.to_string()));
            }
        };
        let writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(error) => {
                let _ = child.kill();
                return Err(TerminalError::Pty(error.to_string()));
            }
        };
        let output = Arc::new(Mutex::new(OutputBuffer::default()));
        let reader_output = Arc::clone(&output);
        if let Err(error) = std::thread::Builder::new()
            .name("pab-terminal-reader".to_owned())
            .spawn(move || {
                let mut buffer = [0_u8; 8 * 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => reader_output.lock().unwrap().append(&buffer[..count]),
                    }
                }
                reader_output.lock().unwrap().ended = true;
            })
        {
            let _ = child.kill();
            return Err(TerminalError::Thread(error));
        }
        Ok(Self {
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            child: Mutex::new(child),
            output,
            exit_seen_at: Mutex::new(None),
        })
    }

    pub fn input(&self, bytes: &[u8]) -> Result<(), TerminalError> {
        if bytes.is_empty() || bytes.len() > MAX_INPUT_BYTES {
            return Err(TerminalError::InvalidInput);
        }
        self.writer
            .lock()
            .unwrap()
            .write_all(bytes)
            .map_err(TerminalError::Io)
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), TerminalError> {
        validate_size(cols, rows)?;
        self.master
            .lock()
            .unwrap()
            .resize(PtySize {
                cols,
                rows,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TerminalError::Pty(error.to_string()))
    }

    pub fn read(&self, offset: u64, max_bytes: usize) -> TerminalOutput {
        let exited = self.has_exited().unwrap_or(false);
        let mut exit_seen_at = self.exit_seen_at.lock().unwrap();
        if exited && exit_seen_at.is_none() {
            *exit_seen_at = Some(Instant::now());
        }
        let mut output = self.output.lock().unwrap();
        let mut chunk = output.read(offset, max_bytes);
        if chunk.bytes.is_empty()
            && exit_seen_at.is_some_and(|at| at.elapsed() >= Duration::from_millis(200))
            && output
                .last_append_at
                .is_none_or(|at| at.elapsed() >= Duration::from_millis(200))
        {
            output.ended = true;
            chunk.ended = chunk.next_offset >= output.retained_from + output.data.len() as u64;
        }
        chunk
    }

    pub fn has_exited(&self) -> Result<bool, TerminalError> {
        self.child
            .lock()
            .unwrap()
            .try_wait()
            .map(|status| status.is_some())
            .map_err(TerminalError::Io)
    }

    pub fn close(&self) -> Result<(), TerminalError> {
        self.child.lock().unwrap().kill().map_err(TerminalError::Io)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if let Ok(child) = self.child.get_mut() {
            let _ = child.kill();
        }
    }
}

fn validate_size(cols: u16, rows: u16) -> Result<(), TerminalError> {
    if !(20..=500).contains(&cols) || !(5..=200).contains(&rows) {
        return Err(TerminalError::InvalidSize);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("terminal size is outside supported bounds")]
    InvalidSize,
    #[error("terminal input is empty or exceeds 4 KiB")]
    InvalidInput,
    #[error("PTY error: {0}")]
    Pty(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("terminal reader could not start: {0}")]
    Thread(std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_buffer_reports_evicted_bytes() {
        let mut buffer = OutputBuffer::default();
        buffer.append(&vec![42; MAX_RETAINED_BYTES + 17]);
        let output = buffer.read(0, 100);
        assert_eq!(output.retained_from, 17);
        assert_eq!(output.offset, 17);
        assert_eq!(output.bytes.len(), 100);
    }

    #[test]
    fn ended_output_remains_readable_across_multiple_chunks() {
        let mut buffer = OutputBuffer::default();
        buffer.append(&vec![42; MAX_READ_BYTES + 17]);
        buffer.ended = true;
        let first = buffer.read(0, MAX_READ_BYTES);
        assert!(!first.ended);
        assert_eq!(first.bytes.len(), MAX_READ_BYTES);
        let second = buffer.read(first.next_offset, MAX_READ_BYTES);
        assert!(second.ended);
        assert_eq!(second.bytes.len(), 17);
    }

    #[test]
    fn terminal_size_and_input_are_bounded() {
        assert!(validate_size(80, 24).is_ok());
        assert!(validate_size(0, 24).is_err());
        assert!(validate_size(80, 0).is_err());
    }

    #[test]
    fn native_pty_captures_shell_output() {
        #[cfg(windows)]
        let (shell, args): (&str, &[&str]) = ("cmd.exe", &["/C", "echo PAB_TERMINAL_SMOKE"]);
        #[cfg(not(windows))]
        let (shell, args): (&str, &[&str]) = ("/bin/sh", &["-c", "echo PAB_TERMINAL_SMOKE"]);

        let session = TerminalSession::start(shell, args, 80, 24).unwrap();
        #[cfg(windows)]
        session.input(b"\x1b[1;1R").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut output = Vec::new();
        loop {
            let next = session.read(output.len() as u64, MAX_READ_BYTES);
            output.extend_from_slice(&next.bytes);
            if next.ended || std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            String::from_utf8_lossy(&output).contains("PAB_TERMINAL_SMOKE"),
            "PTY output: {}",
            String::from_utf8_lossy(&output)
        );
        assert!(session.read(output.len() as u64, MAX_READ_BYTES).ended);
    }

    #[cfg(windows)]
    #[test]
    fn two_interactive_sessions_close_independently() {
        let first = TerminalSession::start("cmd.exe", &["/K"], 80, 24).unwrap();
        let second = TerminalSession::start("cmd.exe", &["/K"], 80, 24).unwrap();
        first.input(b"\x1b[1;1R").unwrap();
        second.input(b"\x1b[1;1R").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        first.input(b"echo PAB_FIRST_SESSION\r\n").unwrap();
        second.input(b"echo PAB_SECOND_SESSION\r\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut first_output = Vec::new();
        let mut second_output = Vec::new();
        while Instant::now() < deadline {
            let first_next = first.read(first_output.len() as u64, MAX_READ_BYTES);
            let second_next = second.read(second_output.len() as u64, MAX_READ_BYTES);
            first_output.extend_from_slice(&first_next.bytes);
            second_output.extend_from_slice(&second_next.bytes);
            if String::from_utf8_lossy(&first_output).contains("PAB_FIRST_SESSION")
                && String::from_utf8_lossy(&second_output).contains("PAB_SECOND_SESSION")
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        first.close().unwrap();
        second.close().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut first_ended = false;
        let mut second_ended = false;
        while Instant::now() < deadline {
            let first_next = first.read(first_output.len() as u64, MAX_READ_BYTES);
            let second_next = second.read(second_output.len() as u64, MAX_READ_BYTES);
            first_output.extend_from_slice(&first_next.bytes);
            second_output.extend_from_slice(&second_next.bytes);
            first_ended |= first_next.ended;
            second_ended |= second_next.ended;
            if first_ended && second_ended {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(first_ended && second_ended);
        assert!(String::from_utf8_lossy(&first_output).contains("PAB_FIRST_SESSION"));
        assert!(String::from_utf8_lossy(&second_output).contains("PAB_SECOND_SESSION"));
    }
}
