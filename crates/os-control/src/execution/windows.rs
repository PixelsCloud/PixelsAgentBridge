use super::*;
use std::{
    ffi::{OsStr, c_void},
    os::windows::ffi::OsStrExt,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Security::{
        Authorization::ConvertSidToStringSidW, GetTokenInformation, LookupAccountSidW, TOKEN_QUERY,
        TOKEN_STATISTICS, TOKEN_USER, TokenSessionId, TokenStatistics, TokenUser,
    },
    System::{
        Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock},
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
        RemoteDesktop::WTSQueryUserToken,
        Threading::{
            CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW,
            GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, PROCESS_INFORMATION,
            ResumeThread, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
        },
    },
    UI::Shell::GetUserProfileDirectoryW,
};

struct Handle(HANDLE);
// SAFETY: owned kernel handles may be transferred across threads. No borrowing
// handle escapes this module and Drop closes each handle exactly once.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut encoded: Vec<_> = value.encode_wide().collect();
    if encoded.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in process argument",
        ));
    }
    encoded.push(0);
    Ok(encoded)
}
fn token(session: u32) -> io::Result<Handle> {
    let mut raw = ptr::null_mut();
    // SAFETY: writable output, current local WTS server. This needs LocalSystem
    // privileges; failure is reported, never replaced with the service token.
    if unsafe { WTSQueryUserToken(session, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Handle(raw))
}
fn token_field<T: Copy>(token: &Handle, kind: i32) -> io::Result<T> {
    let mut value = std::mem::MaybeUninit::<T>::zeroed();
    let mut size = 0;
    // SAFETY: callers pair each information class with its exact fixed-size type.
    if unsafe {
        GetTokenInformation(
            token.0,
            kind,
            value.as_mut_ptr().cast(),
            std::mem::size_of::<T>() as u32,
            &mut size,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if size as usize != std::mem::size_of::<T>() {
        return Err(io::Error::other("unexpected token information length"));
    }
    Ok(unsafe { value.assume_init() })
}
fn identity(token: &Handle) -> io::Result<UserIdentity> {
    let mut needed = 0;
    // SAFETY: size query, no output buffer is supplied.
    unsafe {
        GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 || needed > 65536 {
        return Err(io::Error::other("invalid TokenUser length"));
    }
    // usize storage gives sufficient alignment for TOKEN_USER and SID.
    let mut storage = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            storage.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
    let mut sid_text = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid_text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let account_id = unsafe {
        let mut len = 0;
        while *sid_text.add(len) != 0 {
            len += 1;
        }
        let result = String::from_utf16_lossy(std::slice::from_raw_parts(sid_text, len));
        windows_sys::Win32::Foundation::LocalFree(sid_text.cast());
        result
    };
    let mut name_len = 0;
    let mut domain_len = 0;
    let mut kind = 0;
    unsafe {
        LookupAccountSidW(
            ptr::null(),
            user.User.Sid,
            ptr::null_mut(),
            &mut name_len,
            ptr::null_mut(),
            &mut domain_len,
            &mut kind,
        );
    }
    if name_len == 0 || name_len > 32768 || domain_len > 32768 {
        return Err(io::Error::other("cannot resolve account name"));
    }
    let mut name = vec![0; name_len as usize];
    let mut domain = vec![0; domain_len as usize];
    if unsafe {
        LookupAccountSidW(
            ptr::null(),
            user.User.Sid,
            name.as_mut_ptr(),
            &mut name_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut kind,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let account_name = if domain_len == 0 {
        String::from_utf16_lossy(&name[..name_len as usize])
    } else {
        format!(
            "{}\\{}",
            String::from_utf16_lossy(&domain[..domain_len as usize]),
            String::from_utf16_lossy(&name[..name_len as usize])
        )
    };
    let mut home_len = 0;
    unsafe {
        GetUserProfileDirectoryW(token.0, ptr::null_mut(), &mut home_len);
    }
    if home_len == 0 || home_len > 32768 {
        return Err(io::Error::other("user profile directory unavailable"));
    }
    let mut home = vec![0; home_len as usize];
    if unsafe { GetUserProfileDirectoryW(token.0, home.as_mut_ptr(), &mut home_len) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let end = home.iter().position(|v| *v == 0).unwrap_or(home.len());
    use std::os::windows::ffi::OsStringExt;
    let home = PathBuf::from(OsString::from_wide(&home[..end]));
    let session: u32 = token_field(token, TokenSessionId)?;
    let stat: TOKEN_STATISTICS = token_field(token, TokenStatistics)?;
    Ok(UserIdentity {
        account_id,
        account_name,
        home,
        primary_group: None,
        session_id: Some(session),
        logon_id: Some(format!(
            "{:08x}:{:08x}",
            stat.AuthenticationId.HighPart, stat.AuthenticationId.LowPart
        )),
    })
}

struct Environment(*mut c_void);
impl Drop for Environment {
    fn drop(&mut self) {
        unsafe {
            DestroyEnvironmentBlock(self.0);
        }
    }
}
pub(super) struct PreparedUser {
    identity: UserIdentity,
    token: Handle,
}
impl PreparedUser {
    pub fn for_session(session: u32) -> io::Result<Self> {
        let token = token(session)?;
        let identity = identity(&token)?;
        if identity.session_id != Some(session) {
            return Err(io::Error::other("WTS token session mismatch"));
        }
        Ok(Self { identity, token })
    }
    pub fn identity(&self) -> &UserIdentity {
        &self.identity
    }
    pub fn spawn(
        &self,
        executable: &Path,
        args: &[OsString],
        cwd: &Path,
    ) -> io::Result<UserProcess> {
        let fresh = token(
            self.identity
                .session_id
                .ok_or_else(|| io::Error::other("missing session"))?,
        )?;
        if identity(&fresh)? != self.identity {
            return Err(io::Error::other(
                "execution logon changed; resolve a fresh context",
            ));
        }
        let mut env = ptr::null_mut();
        // No inherited service variables; WTS users already have a loaded profile.
        if unsafe { CreateEnvironmentBlock(&mut env, self.token.0, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let env = Environment(env);
        let application = wide(executable.as_os_str())?;
        let cwd = wide(cwd.as_os_str())?;
        let mut command = command_line(executable.as_os_str(), args)?;
        let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut created: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // The job handle is never inherited. Closing it also stops descendants
        // if the parent crashes, even when the worker itself has already exited.
        let raw_job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw_job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Handle(raw_job);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: all pointers live for the synchronous call, mutable command line,
        // validated primary token. No parent handles inherited, no implicit shell,
        // and no request for the linked elevated token of an administrator.
        if unsafe {
            CreateProcessAsUserW(
                self.token.0,
                application.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW | CREATE_SUSPENDED,
                env.0.cast_const(),
                cwd.as_ptr(),
                &startup,
                &mut created,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let process = Handle(created.hProcess);
        let thread = Handle(created.hThread);
        // No user code runs before containment succeeds. On failure there is no
        // uncontained fallback, and the still-suspended process is terminated.
        if unsafe { AssignProcessToJobObject(job.0, process.0) } == 0 {
            let error = io::Error::last_os_error();
            unsafe {
                TerminateProcess(process.0, 1);
                WaitForSingleObject(process.0, 5000);
            }
            return Err(error);
        }
        if unsafe { ResumeThread(thread.0) } == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        Ok(UserProcess {
            process,
            job,
            pid: created.dwProcessId,
            exited: false,
        })
    }
}

// Windows CRT argv rules, not PowerShell/cmd escaping. Preserve UTF-16, including
// trailing backslashes and embedded quotes, and reject NUL/oversized command lines.
fn command_line(executable: &OsStr, args: &[OsString]) -> io::Result<Vec<u16>> {
    let mut out = Vec::new();
    for arg in std::iter::once(executable).chain(args.iter().map(OsString::as_os_str)) {
        if !out.is_empty() {
            out.push(32);
        }
        out.push(34);
        let mut slashes = 0;
        for c in arg.encode_wide() {
            if c == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "NUL in process argument",
                ));
            }
            if c == 92 {
                slashes += 1;
                continue;
            }
            out.extend(std::iter::repeat_n(
                92,
                if c == 34 { slashes * 2 + 1 } else { slashes },
            ));
            slashes = 0;
            out.push(c);
        }
        out.extend(std::iter::repeat_n(92, slashes * 2));
        out.push(34);
    }
    out.push(0);
    if out.len() > 32767 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "worker command line too long",
        ));
    }
    Ok(out)
}
pub(super) fn current_identity() -> io::Result<UserIdentity> {
    let mut raw = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    identity(&Handle(raw))
}
pub(super) fn process_user_identity(pid: u32) -> io::Result<UserIdentity> {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(io::Error::last_os_error());
    }
    let process = Handle(process);
    let mut raw = ptr::null_mut();
    if unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    identity(&Handle(raw))
}
pub(super) struct UserProcess {
    process: Handle,
    job: Handle,
    pid: u32,
    exited: bool,
}
impl UserProcess {
    pub fn id(&self) -> u32 {
        self.pid
    }
    pub fn try_wait(&mut self) -> io::Result<Option<i32>> {
        match unsafe { WaitForSingleObject(self.process.0, 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                self.exited = true;
                let mut code = 0;
                if unsafe { GetExitCodeProcess(self.process.0, &mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Some(code as i32))
            }
            _ => Err(io::Error::last_os_error()),
        }
    }
    pub fn terminate(&mut self) -> io::Result<()> {
        if unsafe { TerminateJobObject(self.job.0, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if self.try_wait()?.is_none() {
            if unsafe { WaitForSingleObject(self.process.0, 5000) } != WAIT_OBJECT_0 {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "worker termination not confirmed",
                ));
            }
            self.exited = true;
        }
        Ok(())
    }
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
    fn arguments_preserve_quotes_backslashes_empty_and_unicode() {
        let args = ["", "目录 空格\\", "a\"b", "a\\\"b"].map(OsString::from);
        let line = command_line(OsStr::new("C:\\a b\\worker.exe"), &args).unwrap();
        assert_eq!(
            String::from_utf16(&line[..line.len() - 1]).unwrap(),
            "\"C:\\a b\\worker.exe\" \"\" \"目录 空格\\\\\" \"a\\\"b\" \"a\\\\\\\"b\""
        );
        assert!(command_line(OsStr::new("ok"), &[OsString::from("bad\0arg")]).is_err());
    }
    #[test]
    fn current_identity_is_a_native_sid() {
        let identity = current_identity().unwrap();
        assert!(identity.account_id.starts_with("S-1-"));
        assert!(identity.home.is_absolute());
        assert!(identity.logon_id.is_some());
    }
}
