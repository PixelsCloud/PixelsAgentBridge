use std::{
    ffi::{OsStr, OsString, c_void},
    io,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    ptr, slice, thread,
    time::Duration,
};

use pab_agent_core::{DataPaths, DataScope};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT},
    Security::{
        DuplicateTokenEx, GetTokenInformation, SecurityImpersonation, TOKEN_ALL_ACCESS,
        TOKEN_DUPLICATE, TOKEN_LINKED_TOKEN, TOKEN_QUERY, TOKEN_STATISTICS, TokenElevationType,
        TokenElevationTypeLimited, TokenLinkedToken, TokenPrimary, TokenStatistics,
    },
    System::{
        Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock},
        RemoteDesktop::{
            WTS_CURRENT_SERVER_HANDLE, WTS_PROCESS_INFOW, WTS_SESSION_INFOW, WTSActive,
            WTSEnumerateProcessesW, WTSEnumerateSessionsW, WTSFreeMemory,
            WTSGetActiveConsoleSessionId, WTSQuerySessionInformationW, WTSQueryUserToken,
            WTSUserName,
        },
        Threading::{
            CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, OpenProcess, OpenProcessToken,
            PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
            STARTUPINFOW, TerminateProcess, WaitForSingleObject,
        },
    },
};

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: This handle was returned by a successful Win32 open/create call.
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DesktopKind {
    Default,
    Winlogon,
}

impl DesktopKind {
    fn name(self) -> &'static str {
        match self {
            Self::Default => "WinSta0\\Default",
            Self::Winlogon => "WinSta0\\Winlogon",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Target {
    session_id: u32,
    desktop: DesktopKind,
    winlogon_pid: u32,
    user_logon_id: Option<(u32, i32)>,
}

struct Worker {
    target: Target,
    process: OwnedHandle,
}

impl Worker {
    fn is_running(&self) -> bool {
        // SAFETY: The process handle remains owned by this worker.
        unsafe { WaitForSingleObject(self.process.0, 0) == WAIT_TIMEOUT }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // SAFETY: The handle belongs to the process created by this supervisor.
        unsafe { TerminateProcess(self.process.0, 0) };
    }
}

pub fn run() -> Result<(), String> {
    let paths = DataPaths::for_scope(DataScope::Machine).map_err(|error| error.to_string())?;
    pab_logging::init("session-supervisor", paths.root()).map_err(|error| error.to_string())?;

    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut workers = Vec::<Worker>::new();
    loop {
        match active_target() {
            Ok(Some(target)) => {
                let desktops: &[DesktopKind] = if target.desktop == DesktopKind::Default {
                    &[DesktopKind::Default, DesktopKind::Winlogon]
                } else {
                    &[DesktopKind::Winlogon]
                };
                workers.retain(|worker| {
                    worker.target.session_id == target.session_id
                        && worker.target.winlogon_pid == target.winlogon_pid
                        && worker.target.user_logon_id == target.user_logon_id
                        && desktops.contains(&worker.target.desktop)
                        && worker.is_running()
                });
                for &desktop in desktops {
                    let next_target = Target { desktop, ..target };
                    if workers.iter().any(|worker| worker.target == next_target) {
                        continue;
                    }
                    match launch_helper(&executable, next_target) {
                        Ok(next) => {
                            tracing::info!(
                                session_id = target.session_id,
                                desktop = desktop.name(),
                                "session helper launched"
                            );
                            workers.push(next);
                        }
                        Err(error) => {
                            tracing::warn!(
                                session_id = target.session_id,
                                desktop = desktop.name(),
                                %error,
                                "could not launch session helper"
                            );
                        }
                    }
                }
            }
            Ok(None) => workers.clear(),
            Err(error) => tracing::warn!(%error, "could not inspect active Windows session"),
        }
        thread::sleep(Duration::from_secs(3));
    }
}

fn active_target() -> io::Result<Option<Target>> {
    let console = unsafe { WTSGetActiveConsoleSessionId() };
    let mut sessions: *mut WTS_SESSION_INFOW = ptr::null_mut();
    let mut count = 0;
    // SAFETY: WTS writes a process-owned array; it is freed below.
    if unsafe { WTSEnumerateSessionsW(WTS_CURRENT_SERVER_HANDLE, 0, 1, &mut sessions, &mut count) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    let active = if sessions.is_null() || count == 0 {
        Vec::new()
    } else {
        // SAFETY: WTS returned count initialized entries.
        unsafe { slice::from_raw_parts(sessions, count as usize) }
            .iter()
            .filter(|session| session.State == WTSActive)
            .map(|session| session.SessionId)
            .collect::<Vec<_>>()
    };
    // SAFETY: WTS allocated this buffer, including the zero-length case.
    if !sessions.is_null() {
        unsafe { WTSFreeMemory(sessions.cast()) };
    }
    let session_id = active
        .iter()
        .copied()
        .find(|id| *id == console)
        .or_else(|| active.first().copied())
        .or_else(|| (console != u32::MAX).then_some(console));
    let Some(session_id) = session_id else {
        return Ok(None);
    };
    let desktop = if session_has_user(session_id)? {
        DesktopKind::Default
    } else {
        DesktopKind::Winlogon
    };
    let Some(winlogon_pid) = winlogon_process(session_id)? else {
        return Ok(None);
    };
    Ok(Some(Target {
        session_id,
        desktop,
        winlogon_pid,
        user_logon_id: if desktop == DesktopKind::Default {
            Some(logon_id(&user_token(session_id)?)?)
        } else {
            None
        },
    }))
}

fn session_has_user(session_id: u32) -> io::Result<bool> {
    let mut buffer: *mut u16 = ptr::null_mut();
    let mut bytes = 0;
    // SAFETY: WTS allocates the returned UTF-16 buffer.
    if unsafe {
        WTSQuerySessionInformationW(
            WTS_CURRENT_SERVER_HANDLE,
            session_id,
            WTSUserName,
            &mut buffer,
            &mut bytes,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let has_user = !buffer.is_null() && bytes >= 2 && unsafe { *buffer != 0 };
    if !buffer.is_null() {
        // SAFETY: WTS allocated this buffer.
        unsafe { WTSFreeMemory(buffer.cast()) };
    }
    Ok(has_user)
}

fn winlogon_process(session_id: u32) -> io::Result<Option<u32>> {
    let mut processes: *mut WTS_PROCESS_INFOW = ptr::null_mut();
    let mut count = 0;
    // SAFETY: WTS writes a process-owned array; it is freed below.
    if unsafe {
        WTSEnumerateProcessesW(WTS_CURRENT_SERVER_HANDLE, 0, 1, &mut processes, &mut count)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let pid = if processes.is_null() || count == 0 {
        None
    } else {
        // SAFETY: WTS returned count initialized entries.
        unsafe { slice::from_raw_parts(processes, count as usize) }
            .iter()
            .find(|process| {
                process.SessionId == session_id
                    && wide_name_equals(process.pProcessName, "winlogon.exe")
            })
            .map(|process| process.ProcessId)
    };
    if !processes.is_null() {
        // SAFETY: WTS allocated this buffer.
        unsafe { WTSFreeMemory(processes.cast()) };
    }
    Ok(pid)
}

fn wide_name_equals(pointer: *const u16, expected: &str) -> bool {
    if pointer.is_null() {
        return false;
    }
    let mut length = 0usize;
    // SAFETY: WTS process names are null-terminated. The bound rejects malformed names.
    while length < 260 && unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    if length == 260 {
        return false;
    }
    // SAFETY: The pointer contains at least length valid UTF-16 code units.
    let name = String::from_utf16_lossy(unsafe { slice::from_raw_parts(pointer, length) });
    name.eq_ignore_ascii_case(expected)
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn winlogon_token(target: Target) -> io::Result<OwnedHandle> {
    // SAFETY: The PID comes from WTS and the handle is validated before use.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, target.winlogon_pid) };
    if process.is_null() {
        return Err(io::Error::last_os_error());
    }
    let process = OwnedHandle(process);
    let mut image = vec![0u16; 1024];
    let mut size = image.len() as u32;
    // SAFETY: image is writable and size describes its capacity.
    if unsafe { QueryFullProcessImageNameW(process.0, 0, image.as_mut_ptr(), &mut size) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let path = String::from_utf16_lossy(&image[..size as usize]).to_ascii_lowercase();
    if !path.ends_with("\\system32\\winlogon.exe") {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "active session process is not the system Winlogon executable",
        ));
    }

    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: The process handle is valid and token is writable.
    if unsafe { OpenProcessToken(process.0, TOKEN_DUPLICATE | TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = OwnedHandle(token);
    let mut primary: HANDLE = ptr::null_mut();
    // SAFETY: DuplicateTokenEx creates a primary token for this session.
    if unsafe {
        DuplicateTokenEx(
            token.0,
            TOKEN_ALL_ACCESS,
            ptr::null(),
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(OwnedHandle(primary))
}

fn user_token(session_id: u32) -> io::Result<OwnedHandle> {
    let mut token = ptr::null_mut();
    // SAFETY: the SYSTEM supervisor queries the session selected by WTS.
    if unsafe { WTSQueryUserToken(session_id, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(OwnedHandle(token))
}

fn logon_id(token: &OwnedHandle) -> io::Result<(u32, i32)> {
    let mut statistics: TOKEN_STATISTICS = unsafe { std::mem::zeroed() };
    let mut needed = 0;
    // SAFETY: buffer type and length match TokenStatistics.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenStatistics,
            (&mut statistics as *mut TOKEN_STATISTICS).cast(),
            std::mem::size_of_val(&statistics) as u32,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok((
        statistics.AuthenticationId.LowPart,
        statistics.AuthenticationId.HighPart,
    ))
}

fn interactive_token(target: Target) -> io::Result<OwnedHandle> {
    let token = user_token(target.session_id)?;
    if Some(logon_id(&token)?) != target.user_logon_id {
        return Err(io::Error::other(
            "interactive user changed before helper launch",
        ));
    }
    let mut elevation = 0i32;
    let mut needed = 0;
    // SAFETY: TokenElevationType returns a TOKEN_ELEVATION_TYPE (i32).
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenElevationType,
            (&mut elevation as *mut i32).cast(),
            std::mem::size_of_val(&elevation) as u32,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if elevation != TokenElevationTypeLimited {
        return Ok(token);
    }
    // Use the same administrator's linked token to support elevated windows.
    // Standard users have no linked token and remain standard users.
    let mut linked: TOKEN_LINKED_TOKEN = unsafe { std::mem::zeroed() };
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenLinkedToken,
            (&mut linked as *mut TOKEN_LINKED_TOKEN).cast(),
            std::mem::size_of_val(&linked) as u32,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(OwnedHandle(linked.LinkedToken))
}

struct UserEnvironment(*mut c_void);
impl Drop for UserEnvironment {
    fn drop(&mut self) {
        // SAFETY: created by CreateEnvironmentBlock and owned here.
        unsafe { DestroyEnvironmentBlock(self.0) };
    }
}

impl UserEnvironment {
    fn new(token: &OwnedHandle) -> io::Result<Self> {
        let mut block = ptr::null_mut();
        // Do not inherit the supervisor's PAB_DATA_DIR or SYSTEM user profile.
        if unsafe { CreateEnvironmentBlock(&mut block, token.0, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(block))
    }

    fn local_data(&self) -> io::Result<PathBuf> {
        let mut entry = self.0.cast::<u16>();
        // SAFETY: CreateEnvironmentBlock returns double-NUL terminated UTF-16
        // entries; this block is alive for the complete traversal.
        unsafe {
            while *entry != 0 {
                let mut length = 0;
                while *entry.add(length) != 0 {
                    length += 1;
                }
                let units = slice::from_raw_parts(entry, length);
                if let Some(value) = environment_value(units, "LOCALAPPDATA") {
                    let root = PathBuf::from(value);
                    if root.is_absolute() {
                        return Ok(root.join("PixelsAgentBridge"));
                    }
                    break;
                }
                entry = entry.add(length + 1);
            }
        }
        Err(io::Error::other(
            "interactive user LOCALAPPDATA is unavailable",
        ))
    }
}

fn environment_value(entry: &[u16], name: &str) -> Option<OsString> {
    let separator = entry.iter().position(|value| *value == u16::from(b'='))?;
    String::from_utf16_lossy(&entry[..separator])
        .eq_ignore_ascii_case(name)
        .then(|| OsString::from_wide(&entry[separator + 1..]))
}

fn launch_helper(executable: &Path, target: Target) -> io::Result<Worker> {
    // Window properties on a user's desktop must be accessed in that user's
    // logon context. Keep SYSTEM confined to the Winlogon helper.
    let primary = match target.desktop {
        DesktopKind::Default => interactive_token(target)?,
        DesktopKind::Winlogon => winlogon_token(target)?,
    };
    let environment = if target.desktop == DesktopKind::Default {
        let environment = UserEnvironment::new(&primary)?;
        pab_executor::local_ipc::issue_local_access(
            &environment.local_data()?.join("local-access.key"),
        )
        .map_err(io::Error::other)?;
        Some(environment)
    } else {
        None
    };

    let executable_wide = wide(executable.as_os_str());
    let mut command = wide(OsStr::new(&format!(
        "\"{}\" --session-helper --desktop={}",
        executable.display(),
        if target.desktop == DesktopKind::Winlogon {
            "Winlogon"
        } else {
            "Default"
        }
    )));
    let mut desktop = wide(OsStr::new(target.desktop.name()));
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    startup.lpDesktop = desktop.as_mut_ptr();
    let mut created: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: Paths and command line remain alive for the call. The token is a
    // primary token for the selected desktop. Its environment remains alive.
    if unsafe {
        CreateProcessAsUserW(
            primary.0,
            executable_wide.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            if environment.is_some() {
                CREATE_UNICODE_ENVIRONMENT
            } else {
                0
            },
            environment
                .as_ref()
                .map_or(ptr::null(), |block| block.0.cast_const()),
            ptr::null(),
            &startup,
            &mut created,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: The thread handle is no longer needed after process creation.
    unsafe { CloseHandle(created.hThread) };
    Ok(Worker {
        target,
        process: OwnedHandle(created.hProcess),
    })
}
