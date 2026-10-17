//! Per-helper local capability endpoint. No remote authentication or transport
//! changes: only a native capture child in the supervised helper's process group
//! can ask the root exporter for scanout image descriptors.
use super::{LinuxGraphicalSession, LinuxMediaEnvironment};
use std::{
    ffi::OsString,
    fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

pub struct LinuxMediaDmaBroker {
    listener: OwnedFd,
    path: PathBuf,
    target: LinuxMediaEnvironment,
    executable: PathBuf,
    prefix: Vec<OsString>,
    helper_group: u32,
    child: Option<Child>,
}

fn owned(raw: i32) -> io::Result<OwnedFd> {
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: only newly returned descriptors enter this helper.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

impl LinuxMediaDmaBroker {
    pub fn new(
        path: &Path,
        target: LinuxMediaEnvironment,
        executable: &Path,
        prefix: Vec<OsString>,
        helper_group: u32,
    ) -> io::Result<Self> {
        if unsafe { libc::geteuid() } != 0
            || !path.is_absolute()
            || !executable.is_absolute()
            || helper_group <= 1
            || !target.current()
        {
            return Err(io::Error::other("invalid media broker context"));
        }
        // A user can connect but cannot unlink or replace this endpoint.
        let parent = fs::symlink_metadata(
            path.parent()
                .ok_or_else(|| io::Error::other("missing broker parent"))?,
        )?;
        let image = fs::symlink_metadata(executable)?;
        if !parent.is_dir()
            || parent.uid() != 0
            || parent.mode() & 0o022 != 0
            || !image.is_file()
            || image.uid() != 0
            || image.mode() & 0o022 != 0
        {
            return Err(io::Error::other("media broker needs root-owned paths"));
        }
        let listener = owned(unsafe {
            libc::socket(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                0,
            )
        })?;
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        address.sun_family = libc::AF_UNIX as _;
        let bytes = path.as_os_str().as_bytes();
        if bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
            return Err(io::Error::other("invalid media broker path"));
        }
        for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
            *slot = *byte as _;
        }
        if unsafe {
            libc::bind(
                listener.as_raw_fd(),
                &address as *const _ as _,
                std::mem::size_of_val(&address) as _,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let result = Self {
            listener,
            path: path.into(),
            target,
            executable: executable.into(),
            prefix,
            helper_group,
            child: None,
        };
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        std::os::unix::fs::chown(path, Some(result.target.session().uid()), None)?;
        if unsafe { libc::listen(result.listener.as_raw_fd(), 1) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(result)
    }

    /// Called by the same lifecycle loop that observes the active console.
    /// At most one exporter exists, and a retired child is reaped before reuse.
    pub fn poll(&mut self) -> io::Result<()> {
        if !self.target.current() {
            self.stop()?;
            return Err(io::Error::other("media broker session changed"));
        }
        if let Some(child) = &mut self.child {
            if child.try_wait()?.is_some() {
                self.child = None;
            }
        }
        // Leave the next capture queued until the previous exporter is reaped.
        // A capture child's exit can precede its exporter's EOF by one tick.
        if self.child.is_some() {
            return Ok(());
        }
        let raw = unsafe {
            libc::accept4(
                self.listener.as_raw_fd(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                libc::SOCK_CLOEXEC,
            )
        };
        if raw < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::WouldBlock {
                Ok(())
            } else {
                Err(error)
            };
        }
        let connection = owned(raw)?;
        let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
        let mut size = std::mem::size_of_val(&peer) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                connection.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut peer as *mut _ as _,
                &mut size,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        if size as usize != std::mem::size_of_val(&peer)
            || peer.pid <= 1
            || peer.uid != self.target.session().uid()
            || unsafe { libc::getpgid(peer.pid) } != self.helper_group as i32
            || LinuxGraphicalSession::for_media_process(peer.pid as _, peer.uid)
                .as_ref()
                .ok()
                != Some(self.target.session())
        {
            return Err(io::Error::other(
                "media broker peer is outside the supervised helper",
            ));
        }
        let (pid, process) = self.target.pin_process()?;
        // Reserve owned high descriptors before fork. Retain those exact slots
        // in the child instead of overwriting std::process's exec-error pipe.
        let pinned = owned(unsafe { libc::fcntl(process.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 64) })?;
        let channel =
            owned(unsafe { libc::fcntl(connection.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 64) })?;
        let process_fd = pinned.as_raw_fd();
        let socket_fd = channel.as_raw_fd();
        let parent = unsafe { libc::getpid() };
        let mut command = Command::new(&self.executable);
        command
            .args(&self.prefix)
            .args([
                "export-session",
                &pid.to_string(),
                &process_fd.to_string(),
                &socket_fd.to_string(),
            ])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .process_group(0);
        // SAFETY: child-only, async-signal-safe syscalls; captures only scalars.
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(process_fd, libc::F_SETFD, 0) < 0
                    || libc::fcntl(socket_fd, libc::F_SETFD, 0) < 0
                    || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
        self.child = Some(command.spawn()?);
        // Retire immediately if the compositor or active seat changed at fork.
        if !self.target.current() {
            self.stop()?;
        }
        Ok(())
    }

    pub fn stop(&mut self) -> io::Result<()> {
        if let Some(mut child) = self.child.take() {
            if child.try_wait()?.is_none() {
                if let Err(error) = child.kill() {
                    self.child = Some(child);
                    return Err(error);
                }
            }
            if let Err(error) = child.wait() {
                self.child = Some(child);
                return Err(error);
            }
        }
        Ok(())
    }
}
impl Drop for LinuxMediaDmaBroker {
    fn drop(&mut self) {
        let _ = self.stop();
        // The containing directory remains root-owned until this guard drops.
        let _ = fs::remove_file(&self.path);
    }
}
