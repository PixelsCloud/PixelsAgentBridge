//! Native user-worker launch primitives. These do not grant remote authorization.
//!
//! Callers must bind a resolved identity to the device, operator and request before
//! launching. No thread impersonation or process-wide identity changes are used.
//! Desktop bootstrap/session routing is deliberately separate from account identity.
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

pub mod channel;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
use unix as native;
#[cfg(windows)]
use windows as native;

/// Actual native account facts, never values inferred from environment variables.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserIdentity {
    pub account_id: String,
    pub account_name: String,
    pub home: PathBuf,
    pub primary_group: Option<u32>,
    pub session_id: Option<u32>,
    pub logon_id: Option<String>,
}

/// Observe a live desktop helper through native process credentials, not fields
/// supplied by its registration message. The caller must also prove IPC peer PID.
pub fn process_user_identity(pid: u32) -> io::Result<UserIdentity> {
    let before = crate::process_identity(pid).map_err(io::Error::other)?;
    let identity = native::process_user_identity(pid)?;
    if crate::process_identity(pid).map_err(io::Error::other)? != before {
        return Err(io::Error::other(
            "process identity changed during observation",
        ));
    }
    Ok(identity)
}

impl UserIdentity {
    pub fn observation(
        &self,
        mode: pab_protocol::ExecutionMode,
        environment_source: pab_protocol::ExecutionEnvironmentSource,
    ) -> io::Result<pab_protocol::ExecutionIdentity> {
        let observation = pab_protocol::ExecutionIdentity {
            mode,
            account_id: self.account_id.clone(),
            account_name: self.account_name.clone(),
            home: self
                .home
                .to_str()
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "user home is not UTF-8")
                })?
                .to_owned(),
            primary_group: self.primary_group,
            session_id: self.session_id.map(|v| v.to_string()),
            logon_id: self.logon_id.clone(),
            environment_source,
        };
        observation.validate().map_err(io::Error::other)?;
        Ok(observation)
    }
}

/// Captures a verified account/token. A stale reference must be rejected by callers
/// before constructing this value; spawn rechecks the captured native facts.
pub struct PreparedUser(native::PreparedUser);

impl PreparedUser {
    /// Refresh native facts rather than trusting a persisted account name or UID.
    pub fn from_observation(expected: &pab_protocol::ExecutionIdentity) -> io::Result<Self> {
        expected.validate().map_err(io::Error::other)?;
        if expected.mode != pab_protocol::ExecutionMode::User {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "expected native user execution",
            ));
        }
        #[cfg(unix)]
        let value = Self::for_uid(
            expected
                .account_id
                .strip_prefix("uid:")
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| io::Error::other("invalid execution UID"))?,
        )?;
        #[cfg(windows)]
        let value = Self::for_session(
            expected
                .session_id
                .as_ref()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| io::Error::other("missing execution session"))?,
        )?;
        let observed = value.identity().observation(
            pab_protocol::ExecutionMode::User,
            pab_protocol::ExecutionEnvironmentSource::NativeAccount,
        )?;
        if &observed != expected {
            return Err(io::Error::other(
                "execution account/session changed; query contexts again",
            ));
        }
        Ok(value)
    }
    #[cfg(unix)]
    pub fn for_uid(uid: u32) -> io::Result<Self> {
        native::PreparedUser::for_uid(uid).map(Self)
    }

    #[cfg(windows)]
    pub fn for_session(session_id: u32) -> io::Result<Self> {
        native::PreparedUser::for_session(session_id).map(Self)
    }

    pub fn identity(&self) -> &UserIdentity {
        self.0.identity()
    }

    /// Launch a trusted, absolute worker binary with a clean user environment.
    /// This is not a generic shell runner. It does not inherit service secrets,
    /// stdio handles or network connections. The child performs its own file/PTY
    /// operations after the OS has established its account.
    pub fn spawn(
        &self,
        executable: &Path,
        args: &[OsString],
        cwd: &Path,
    ) -> io::Result<UserProcess> {
        if !executable.is_absolute() || !cwd.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "worker executable and cwd must be absolute",
            ));
        }
        self.0.spawn(executable, args, cwd).map(UserProcess)
    }
}

/// Owned worker, killed and reaped when dropped while still running.
/// Windows owns a kill-on-close Job Object. Unix owns a dedicated process group;
/// descendants starting another session (such as a PTY) additionally require
/// explicit worker cleanup. Parent loss on Unix is detected by the IPC worker.
pub struct UserProcess(native::UserProcess);
impl UserProcess {
    pub fn id(&self) -> u32 {
        self.0.id()
    }
    pub fn try_wait(&mut self) -> io::Result<Option<i32>> {
        self.0.try_wait()
    }
    pub fn terminate(&mut self) -> io::Result<()> {
        self.0.terminate()
    }
}

pub fn current_identity() -> io::Result<UserIdentity> {
    native::current_identity()
}

/// Last-resort cleanup when the internal worker loses its parent channel. This
/// must only run inside a disposable worker in its own dedicated process group.
/// Windows is contained by the parent's kill-on-close job even on parent crash.
pub fn stop_disconnected_worker_group() -> io::Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: kernel facts guard against ever signalling a caller's group.
        let pid = unsafe { libc::getpid() };
        if unsafe { libc::getpgrp() } != pid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "not an isolated worker process group",
            ));
        }
        if unsafe { libc::killpg(pid, libc::SIGKILL) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
