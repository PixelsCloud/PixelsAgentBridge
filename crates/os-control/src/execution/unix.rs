use super::*;
#[path = "unix_environment.rs"]
mod environment;
use std::{
    ffi::{CStr, CString, OsStr},
    os::unix::{ffi::OsStrExt, process::CommandExt},
    process::{Child, Command, Stdio},
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Account {
    identity: UserIdentity,
    uid: libc::uid_t,
    gid: libc::gid_t,
    shell: OsString,
}

fn lookup(uid: u32) -> io::Result<Account> {
    let mut size = 16384;
    loop {
        let mut buffer = vec![0u8; size];
        // SAFETY: getpwuid_r initializes the record and points strings into our
        // live caller-owned buffer. Copy all fields before freeing that buffer.
        let mut record: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result = std::ptr::null_mut();
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                &mut record,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && size < 1024 * 1024 {
            size *= 2;
            continue;
        }
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status));
        }
        if result.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "execution account no longer exists",
            ));
        }
        let copy = |ptr: *const libc::c_char| -> io::Result<OsString> {
            if ptr.is_null() {
                return Err(io::Error::other("account record has a missing field"));
            }
            // SAFETY: successful reentrant lookup supplies NUL-terminated fields.
            Ok(OsStr::from_bytes(unsafe { CStr::from_ptr(ptr) }.to_bytes()).to_owned())
        };
        let name = copy(record.pw_name)?
            .into_string()
            .map_err(|_| io::Error::other("account name is not UTF-8"))?;
        let home = PathBuf::from(copy(record.pw_dir)?);
        if name.is_empty() || !home.is_absolute() {
            return Err(io::Error::other("account lacks an absolute home directory"));
        }
        return Ok(Account {
            identity: UserIdentity {
                account_id: format!("uid:{uid}"),
                account_name: name,
                home,
                primary_group: Some(record.pw_gid),
                session_id: None,
                logon_id: None,
            },
            uid: record.pw_uid,
            gid: record.pw_gid,
            shell: copy(record.pw_shell)?,
        });
    }
}

fn groups(account: &Account) -> io::Result<Vec<libc::gid_t>> {
    let name = CString::new(account.identity.account_name.as_bytes())?;
    let mut size = 32;
    loop {
        let mut values = vec![0; size as usize];
        let mut count = size;
        // SAFETY: writable vector contains count gid_t slots; getgrouplist reports
        // the required count on failure. Unlike initgroups this changes no identity.
        let status = unsafe {
            // Darwin declares these as int rather than gid_t; the bit pattern
            // represents the same native group identifier on both platforms.
            libc::getgrouplist(
                name.as_ptr(),
                account.gid as _,
                values.as_mut_ptr(),
                &mut count,
            )
        };
        if status >= 0 && count >= 0 && count <= size {
            values.truncate(count as usize);
            values.sort_unstable();
            values.dedup();
            return Ok(values.into_iter().map(|v| v as libc::gid_t).collect());
        }
        if count <= size || count > 65536 {
            return Err(io::Error::other("cannot resolve supplementary groups"));
        }
        size = count;
    }
}

pub(super) struct PreparedUser {
    account: Account,
    groups: Vec<libc::gid_t>,
}
impl PreparedUser {
    pub fn for_uid(uid: u32) -> io::Result<Self> {
        // SAFETY: pure identity reads.
        let effective = unsafe { libc::geteuid() };
        if effective != 0 && effective != uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "service cannot switch to this account",
            ));
        }
        let account = lookup(uid)?;
        let groups = groups(&account)?;
        Ok(Self { account, groups })
    }
    pub fn identity(&self) -> &UserIdentity {
        &self.account.identity
    }
    pub fn spawn(
        &self,
        executable: &Path,
        args: &[OsString],
        cwd: &Path,
    ) -> io::Result<UserProcess> {
        if lookup(self.account.uid)? != self.account || groups(&self.account)? != self.groups {
            return Err(io::Error::other(
                "execution account changed; resolve a fresh context",
            ));
        }
        // Build everything before fork. The child hook calls only async-signal-safe
        // setgroups, never NSS, initgroups, allocation or Rust logging after fork.
        let destination = CString::new(cwd.as_os_str().as_bytes())?;
        let mut command = Command::new(executable);
        command
            .args(args)
            .process_group(0)
            .current_dir("/")
            .env_clear()
            .env("HOME", &self.account.identity.home)
            .env("USER", &self.account.identity.account_name)
            .env("LOGNAME", &self.account.identity.account_name)
            .env("SHELL", &self.account.shell)
            .env(
                "PATH",
                environment::search_path(&self.account.identity.home)?,
            )
            .env("TMPDIR", "/tmp")
            .env(
                "LANG",
                if cfg!(target_os = "macos") {
                    "en_US.UTF-8"
                } else {
                    "C.UTF-8"
                },
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(socket) = environment::ssh_agent(self.account.uid) {
            command.env("SSH_AUTH_SOCK", socket);
        }
        // SAFETY: pure identity read; permissions are still enforced by the kernel.
        let privileged = unsafe { libc::geteuid() } == 0;
        if !privileged
            && (unsafe { libc::getuid() } != self.account.uid
                || unsafe { libc::getgid() } != self.account.gid
                || unsafe { libc::getegid() } != self.account.gid)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "current real account/group does not match selected user",
            ));
        }
        {
            let supplementary = self.groups.clone();
            // Rust applies uid/gid BEFORE pre_exec callbacks, so supplementary
            // groups and identity must all be changed inside the one hook.
            let gid = self.account.gid;
            let uid = self.account.uid;
            // SAFETY: only async-signal-safe syscalls and error construction in
            // this child-only hook. The multithreaded parent never changes identity.
            unsafe {
                command.pre_exec(move || {
                    if privileged
                        && (libc::setgroups(supplementary.len() as _, supplementary.as_ptr()) != 0
                            || libc::setgid(gid) != 0
                            || libc::setuid(uid) != 0)
                    {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::getuid() != uid
                        || libc::geteuid() != uid
                        || libc::getgid() != gid
                        || libc::getegid() != gid
                    {
                        return Err(io::Error::from_raw_os_error(libc::EPERM));
                    }
                    if libc::chdir(destination.as_ptr()) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        // chdir happens in the hook AFTER dropping privileges. A root-only cwd
        // must fail, even if the service itself can traverse that directory.
        command.spawn().map(|child| UserProcess {
            child,
            exited: false,
            group_stopped: false,
        })
    }
}

pub(super) fn current_identity() -> io::Result<UserIdentity> {
    // SAFETY: pure identity read; environment variables are deliberately ignored.
    let mut identity = lookup(unsafe { libc::geteuid() })?.identity;
    identity.primary_group = Some(unsafe { libc::getegid() });
    Ok(identity)
}
pub(super) fn process_user_identity(pid: u32) -> io::Result<UserIdentity> {
    #[cfg(target_os = "macos")]
    {
        let token = crate::macos::audit_token(pid).map_err(io::Error::other)?;
        let mut identity = lookup(token[1])?.identity;
        identity.primary_group = Some(token[2]);
        identity.session_id = Some(token[6]);
        Ok(identity)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "desktop helper observation is unavailable on the headless product",
        ))
    }
}
pub(super) struct UserProcess {
    child: Child,
    exited: bool,
    group_stopped: bool,
}
impl UserProcess {
    pub fn id(&self) -> u32 {
        self.child.id()
    }
    pub fn try_wait(&mut self) -> io::Result<Option<i32>> {
        if !self.exited && self.exit_observed()? {
            self.stop_group(true)?;
        }
        let status = self.child.try_wait()?;
        self.exited |= status.is_some();
        Ok(status.map(|v| v.code().unwrap_or(-1)))
    }
    fn exit_observed(&self) -> io::Result<bool> {
        // Observe without reaping. This reserves the PID while checking/killing
        // the group, including a child that exits during an explicit terminate.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id() as _,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(info.si_signo != 0)
    }
    pub fn terminate(&mut self) -> io::Result<()> {
        if !self.exited {
            self.stop_group(false)?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while self.try_wait()?.is_none() {
                if std::time::Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "worker termination not confirmed",
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        Ok(())
    }
    fn stop_group(&mut self, child_exited: bool) -> io::Result<()> {
        if self.group_stopped {
            return Ok(());
        }
        // The worker started a dedicated process group before dropping identity.
        // Descendants that start another session need explicit worker cleanup.
        if unsafe { libc::killpg(self.child.id() as _, libc::SIGKILL) } != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                #[cfg(target_os = "macos")]
                if error.raw_os_error() == Some(libc::EPERM) {
                    // Darwin can stop considering an exiting leader signalable
                    // just before waitid reports its exit. Keep its PID reserved
                    // and briefly wait for that proof; never waive EPERM for a
                    // still-running leader or any remaining live descendant.
                    let deadline =
                        std::time::Instant::now() + std::time::Duration::from_millis(100);
                    let mut exited = child_exited;
                    while !exited {
                        exited = self.exit_observed()?;
                        if exited || std::time::Instant::now() >= deadline {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    if exited && !mac_group_has_live_members(self.child.id())? {
                        self.group_stopped = true;
                        return Ok(());
                    }
                }
                return Err(io::Error::new(
                    error.kind(),
                    format!("worker process-group termination failed: {error}"),
                ));
            }
        }
        let _ = child_exited;
        self.group_stopped = true;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn mac_group_has_live_members(group: u32) -> io::Result<bool> {
    // Darwin killpg skips zombies and returns EPERM for a zombie-only group.
    // Verify the condition; never reinterpret a real permission denial as success.
    const PROC_PGRP_ONLY: u32 = 2;
    let mut pids = vec![0i32; 4096];
    let capacity = (pids.len() * std::mem::size_of::<i32>()) as i32;
    let bytes =
        unsafe { libc::proc_listpids(PROC_PGRP_ONLY, group, pids.as_mut_ptr().cast(), capacity) };
    if bytes < 0 || bytes >= capacity {
        return Err(io::Error::other("cannot verify worker process group"));
    }
    for pid in pids
        .into_iter()
        .take(bytes as usize / std::mem::size_of::<i32>())
        .filter(|v| *v > 0 && *v as u32 != group)
    {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of_val(&info) as i32;
        let n = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        if n != size {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                continue;
            }
            return Err(io::Error::new(
                error.kind(),
                format!("cannot inspect remaining worker group member {pid}: {error}"),
            ));
        }
        if info.pbi_pgid == group && info.pbi_status != libc::SZOMB {
            return Ok(true);
        }
    }
    Ok(false)
}
impl Drop for UserProcess {
    fn drop(&mut self) {
        if !self.exited {
            let _ = self.terminate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_kernel_account_and_rejects_nonexistent_account() {
        let current = current_identity().unwrap();
        assert_eq!(
            current.account_id,
            format!("uid:{}", unsafe { libc::geteuid() })
        );
        assert!(lookup(u32::MAX).is_err());
    }
    #[test]
    fn stale_account_cannot_spawn() {
        let mut selected = PreparedUser::for_uid(unsafe { libc::geteuid() }).unwrap();
        selected.account.identity.account_name.push_str("-changed");
        assert!(
            selected
                .spawn(Path::new("/bin/true"), &[], Path::new("/"))
                .is_err()
        );
    }
    #[test]
    fn explicit_terminate_reaps_an_already_exited_group_leader() {
        let child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .process_group(0)
            .spawn()
            .unwrap();
        let mut process = UserProcess {
            child,
            exited: false,
            group_stopped: false,
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !process.exit_observed().unwrap() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Do not call try_wait first: it already handles zombie-only groups.
        // Explicit termination must independently handle the exit/kill race.
        process.terminate().unwrap();
        assert!(process.exited && process.group_stopped);
        process.terminate().unwrap();
    }
}
