//! A local, owned process lifetime for long-running media. Unlike the public
//! process-operation token, this handle lives until its owner releases it.
use super::UserIdentity;
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fd::OwnedFd,
    process::{Pid, PidfdFlags, pidfd_open},
};
use std::{
    collections::HashMap,
    io,
    sync::{Arc, LazyLock, Mutex, Weak},
};

#[derive(Debug)]
struct Process {
    pid: u32,
    fd: OwnedFd,
}
impl Process {
    fn current(&self) -> bool {
        let mut descriptors = [PollFd::new(&self.fd, PollFlags::IN)];
        poll(
            &mut descriptors,
            Some(&Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            }),
        )
        .is_ok_and(|_| descriptors[0].revents().is_empty())
    }
}

static WATCHED: LazyLock<Mutex<HashMap<u32, Weak<Process>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Pins the kernel process, with no PID-only fallback or credential renewal.
/// Clones and repeated observations share one fd while an observer is alive.
#[derive(Clone, Debug)]
pub struct LinuxProcessWatch(Arc<Process>);
impl PartialEq for LinuxProcessWatch {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for LinuxProcessWatch {}
impl LinuxProcessWatch {
    pub fn observe(pid: u32) -> io::Result<Self> {
        let mut watched = WATCHED
            .lock()
            .map_err(|_| io::Error::other("process watch unavailable"))?;
        // Weak entries do not retain descriptors after the final media owner.
        watched.retain(|_, process| process.strong_count() != 0);
        if let Some(process) = watched.get(&pid).and_then(Weak::upgrade)
            && process.current()
        {
            return Ok(Self(process));
        }
        let native_pid = i32::try_from(pid)
            .ok()
            .and_then(Pid::from_raw)
            .ok_or_else(|| io::Error::other("invalid process PID"))?;
        let process = Arc::new(Process {
            pid,
            fd: pidfd_open(native_pid, PidfdFlags::empty())?,
        });
        if !process.current() {
            return Err(io::Error::other("process already exited"));
        }
        watched.insert(pid, Arc::downgrade(&process));
        Ok(Self(process))
    }
    pub fn current(&self) -> bool {
        self.0.current()
    }
    /// Read native credentials between checks of the same retained process.
    pub fn user_identity(&self) -> io::Result<UserIdentity> {
        if !self.current() {
            return Err(io::Error::other("watched process exited"));
        }
        let identity = super::native::process_user_identity(self.0.pid)?;
        if !self.current() {
            return Err(io::Error::other(
                "watched process exited during observation",
            ));
        }
        Ok(identity)
    }
    pub fn duplicate_fd(&self) -> io::Result<OwnedFd> {
        if !self.current() {
            return Err(io::Error::other("watched process exited"));
        }
        Ok(rustix::io::dup(&self.0.fd)?)
    }
}
