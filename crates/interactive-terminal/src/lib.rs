//! A streaming PTY owned ONLY by a disposable worker. Never instantiate in the
//! Executor service: Windows ConPTY destruction may block after Shell exit.
use pab_protocol::{
    DESKTOP_TERMINAL_CHUNK, DESKTOP_TERMINAL_INPUT, validate_desktop_terminal_size,
};
use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use std::{
    io::{self, Read, Write},
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;

pub enum PtyOutput {
    Bytes(Vec<u8>),
    End,
    Error(String),
}
pub struct InteractivePty {
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    #[cfg(unix)]
    group_closed: std::sync::atomic::AtomicBool,
}
impl InteractivePty {
    pub fn start(
        shell: &str,
        args: &[String],
        cwd: &Path,
        cols: u16,
        rows: u16,
    ) -> io::Result<(Arc<Self>, mpsc::Receiver<PtyOutput>)> {
        validate_desktop_terminal_size(cols, rows).map_err(io::Error::other)?;
        if !cwd.is_absolute() || !cwd.is_dir() {
            return Err(io::Error::other("terminal home directory unavailable"));
        }
        let pair = NativePtySystem::default()
            .openpty(PtySize {
                cols,
                rows,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io::Error::other)?;
        let mut command = CommandBuilder::new(shell);
        command.args(args);
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(io::Error::other)?;
        drop(pair.slave);
        let mut reader = match pair.master.try_clone_reader() {
            Ok(r) => r,
            Err(e) => {
                let _ = child.kill();
                return Err(io::Error::other(e));
            }
        };
        let writer = match pair.master.take_writer() {
            Ok(w) => Arc::new(Mutex::new(w)),
            Err(e) => {
                let _ = child.kill();
                return Err(io::Error::other(e));
            }
        };
        let (tx, rx) = mpsc::channel(4);
        let session = Arc::new(Self {
            master: Mutex::new(Some(pair.master)),
            writer,
            child: Mutex::new(child),
            #[cfg(unix)]
            group_closed: std::sync::atomic::AtomicBool::new(false),
        });
        let cleanup = Arc::downgrade(&session);
        std::thread::Builder::new()
            .name("desktop-pty-output".into())
            .spawn(move || {
                let mut buffer = [0; DESKTOP_TERMINAL_CHUNK];
                let mut abandoned = false;
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => {
                            let _ = tx.blocking_send(PtyOutput::End);
                            break;
                        }
                        Ok(n) => {
                            if tx
                                .blocking_send(PtyOutput::Bytes(buffer[..n].to_vec()))
                                .is_err()
                            {
                                abandoned = true;
                                break;
                            }
                        }
                        Err(e) => {
                            #[cfg(unix)]
                            if e.raw_os_error() == Some(libc::EIO) {
                                // A Unix master returns EIO when its last slave
                                // closes. The worker still checks real Shell exit.
                                let _ = tx.blocking_send(PtyOutput::End);
                                break;
                            }
                            let _ = tx.blocking_send(PtyOutput::Error(e.to_string()));
                            break;
                        }
                    }
                }
                // Reader loss is not a reason to leave an orphan Shell.
                if abandoned && let Some(pty) = cleanup.upgrade() {
                    let _ = pty.kill();
                }
            })?;
        Ok((session, rx))
    }
    pub fn input(&self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() || bytes.len() > DESKTOP_TERMINAL_INPUT {
            return Err(io::Error::other("invalid terminal input size"));
        }
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("PTY input lock poisoned"))?;
        writer.write_all(bytes)?;
        writer.flush()
    }
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        validate_desktop_terminal_size(cols, rows).map_err(io::Error::other)?;
        self.master
            .lock()
            .map_err(|_| io::Error::other("PTY resize lock poisoned"))?
            .as_ref()
            .ok_or_else(|| io::Error::other("Shell exited"))?
            .resize(PtySize {
                cols,
                rows,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io::Error::other)
    }
    /// Close ConPTY on its own thread after actual Shell exit. The reader drains
    /// the pipe to EOF; no quiet-period guess may truncate the final output.
    pub fn drain_after_exit(&self) -> io::Result<()> {
        if let Some(master) = self
            .master
            .lock()
            .map_err(|_| io::Error::other("PTY drain lock poisoned"))?
            .take()
        {
            std::thread::Builder::new()
                .name("desktop-pty-drain".into())
                .spawn(move || drop(master))?;
        }
        Ok(())
    }
    pub fn exit_code(&self) -> io::Result<Option<u32>> {
        let mut child = self
            .child
            .lock()
            .map_err(|_| io::Error::other("PTY child lock poisoned"))?;
        #[cfg(unix)]
        if !self.group_closed.load(std::sync::atomic::Ordering::Acquire) {
            // Preserve the leader's waitable identity until group cleanup. A
            // reaped PID/PGID must never be signalled after possible PID reuse.
            let pid = child
                .process_id()
                .ok_or_else(|| io::Error::other("PTY child PID unavailable"))?;
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            loop {
                if unsafe {
                    libc::waitid(
                        libc::P_PID,
                        pid as libc::id_t,
                        &mut info,
                        libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                    )
                } == 0
                {
                    break;
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
            if unsafe { info.si_pid() } == 0 {
                return Ok(None);
            }
            return Ok(Some(if info.si_code == libc::CLD_EXITED {
                unsafe { info.si_status() as u32 }
            } else {
                unsafe { 128 + info.si_status() as u32 }
            }));
        }
        Ok(child.try_wait()?.map(|s| s.exit_code()))
    }
    pub fn kill(&self) -> io::Result<()> {
        let mut child = self
            .child
            .lock()
            .map_err(|_| io::Error::other("PTY child lock poisoned"))?;
        #[cfg(unix)]
        {
            if self.group_closed.load(std::sync::atomic::Ordering::Acquire) {
                return Ok(());
            }
            let pid = child
                .process_id()
                .ok_or_else(|| io::Error::other("PTY child PID unavailable"))?;
            // portable-pty creates a separate session. Stop that owned group,
            // including grandchildren which outlived their foreground parent.
            if unsafe { libc::kill(-(pid as i32), libc::SIGKILL) } != 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    #[cfg(target_os = "macos")]
                    if error.raw_os_error() == Some(libc::EPERM) && mac_zombie_only_group(pid)? {
                        // Darwin reports EPERM for zombie-only groups, even to
                        // their parent. The unreaped leader pins the PID; native
                        // process inspection must prove there are no live peers.
                        self.group_closed
                            .store(true, std::sync::atomic::Ordering::Release);
                        let _ = child.try_wait()?;
                        return Ok(());
                    }
                    return Err(error);
                }
            }
            self.group_closed
                .store(true, std::sync::atomic::Ordering::Release);
            let _ = child.kill();
            let _ = child.try_wait()?;
            return Ok(());
        }
        #[cfg(windows)]
        child.kill()
    }
}

#[cfg(target_os = "macos")]
fn mac_zombie_only_group(leader: u32) -> io::Result<bool> {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::waitid(
            libc::P_PID,
            leader as _,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if info.si_signo == 0 {
        return Ok(false);
    }
    const PROC_PGRP_ONLY: u32 = 2;
    let mut pids = vec![0_i32; 4096];
    let capacity = (pids.len() * std::mem::size_of::<i32>()) as i32;
    let bytes =
        unsafe { libc::proc_listpids(PROC_PGRP_ONLY, leader, pids.as_mut_ptr().cast(), capacity) };
    if bytes < 0 || bytes >= capacity {
        return Err(io::Error::other("cannot verify owned PTY group"));
    }
    for pid in pids
        .into_iter()
        .take(bytes as usize / std::mem::size_of::<i32>())
        .filter(|pid| *pid > 0 && *pid as u32 != leader)
    {
        let mut process: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of_val(&process) as i32;
        if unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut process as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        } != size
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                continue;
            }
            return Err(error);
        }
        if process.pbi_pgid == leader && process.pbi_status != libc::SZOMB {
            return Ok(false);
        }
    }
    Ok(true)
}
impl Drop for InteractivePty {
    fn drop(&mut self) {
        // Unix PTYs have a separate group from their disposable worker. Clean
        // it even when a malformed control frame or parent IPC loss ends serve.
        let _ = self.kill();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn actual_unix_pty_drains_unicode_and_tail_without_reaping_leader_early() {
        // The real native worker launches with a UTF-8 LANG in its clean
        // account environment. launchd test jobs otherwise start in locale C,
        // where macOS's old /bin/sh readline treats UTF-8 bytes as Meta keys.
        let lang = if cfg!(target_os = "macos") {
            "LANG=en_US.UTF-8"
        } else {
            "LANG=C.UTF-8"
        };
        let (pty, mut output) = InteractivePty::start(
            "/usr/bin/env",
            &[lang.into(), "/bin/sh".into(), "-i".into()],
            &std::env::temp_dir(),
            100,
            40,
        )
        .unwrap();
        pty.input("printf 'PTY_中文_🌍\\n'; head -c 700000 /dev/zero | tr '\\000' X; printf '\\nPTY_FINAL\\n'; exit 7\n".as_bytes()).unwrap();
        let mut bytes = Vec::new();
        let drained = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                // Match the worker's lifecycle: observe actual exit and close
                // the master while the independent reader drains its tail.
                let item = tokio::select! {
                    item = output.recv() => item.unwrap(),
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {
                        if pty.exit_code().unwrap().is_some() { pty.drain_after_exit().unwrap(); }
                        continue;
                    }
                };
                match item {
                    PtyOutput::Bytes(chunk) => bytes.extend(chunk),
                    PtyOutput::End => break,
                    PtyOutput::Error(e) => panic!("PTY read failed: {e}"),
                }
            }
        })
        .await;
        assert!(
            drained.is_ok(),
            "PTY drain timeout: exit={:?}, bytes={}, tail={:?}",
            pty.exit_code(),
            bytes.len(),
            String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(600)..])
        );
        assert!(bytes.len() > 700000);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("PTY_中文_🌍"));
        assert!(text.contains("PTY_FINAL"));
        let mut code = None;
        for _ in 0..100 {
            code = pty.exit_code().unwrap();
            if code.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(code, Some(7));
        // A second observation sees the same waitable process. Cleanup is
        // idempotent and may signal only this owned group before reaping it.
        assert_eq!(pty.exit_code().unwrap(), Some(7));
        pty.kill().unwrap();
        pty.kill().unwrap();
    }

    #[tokio::test]
    async fn receiver_loss_stops_owned_shell_and_unrelated_process_survives() {
        let mut unrelated = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let (pty, output) =
            InteractivePty::start("/bin/sh", &["-i".into()], &std::env::temp_dir(), 100, 40)
                .unwrap();
        drop(output);
        pty.input(b"while :; do printf X; done\n").unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if pty.exit_code().unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        let survived = unrelated.try_wait().unwrap().is_none();
        unrelated.kill().unwrap();
        let _ = unrelated.wait();
        assert!(
            ended.is_ok(),
            "abandoned consumer left the owned Shell alive"
        );
        assert!(survived, "cleanup affected an unrelated process");
    }
}
