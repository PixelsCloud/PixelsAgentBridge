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
/// Descendant/PTY cleanup is the worker protocol's responsibility; this handle
/// alone does not promise process-tree cancellation.
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
