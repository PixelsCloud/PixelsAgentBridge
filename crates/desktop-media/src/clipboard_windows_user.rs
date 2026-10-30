//! Keep clipboard access in the interactive user's process identity. The
//! authenticated SYSTEM media helper remains the sole owner of its private pipe.
use std::{ffi::OsStr, io, os::windows::ffi::OsStrExt, ptr};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE,
        WAIT_OBJECT_0,
    },
    Security::{
        GetTokenInformation, IsWellKnownSid, TOKEN_QUERY, TokenSessionId, TokenUser,
        WinLocalSystemSid,
    },
    System::{
        Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
        Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock},
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        },
        RemoteDesktop::{ProcessIdToSessionId, WTSQueryUserToken},
        StationsAndDesktops::{GetThreadDesktop, GetUserObjectInformationW, UOI_NAME},
        Threading::{
            CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW,
            DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
            GetCurrentProcessId, GetCurrentThreadId, GetExitCodeProcess,
            InitializeProcThreadAttributeList, OpenProcessToken, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW,
            TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
        },
    },
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
struct Environment(*mut std::ffi::c_void);
impl Drop for Environment {
    fn drop(&mut self) {
        unsafe {
            DestroyEnvironmentBlock(self.0);
        }
    }
}
struct Attributes(Vec<usize>);
impl Attributes {
    fn pointer(&mut self) -> *mut std::ffi::c_void {
        self.0.as_mut_ptr().cast()
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.pointer().cast());
        }
    }
}

fn token_is_system(token: HANDLE) -> io::Result<bool> {
    let mut size = 0;
    unsafe {
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
    }
    if size == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut data = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
    if unsafe { GetTokenInformation(token, TokenUser, data.as_mut_ptr().cast(), size, &mut size) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    let user = unsafe {
        &*data
            .as_ptr()
            .cast::<windows_sys::Win32::Security::TOKEN_USER>()
    };
    Ok(unsafe { IsWellKnownSid(user.User.Sid, WinLocalSystemSid) } != 0)
}

// Windows CRT quoting, independent of any shell; preserve UTF-16 and trailing
// backslashes. Arguments are the current dedicated clipboard worker invocation.
fn command_line(arguments: impl IntoIterator<Item = impl AsRef<OsStr>>) -> io::Result<Vec<u16>> {
    let mut out = Vec::new();
    for argument in arguments {
        if !out.is_empty() {
            out.push(32);
        }
        out.push(34);
        let mut slashes = 0;
        for c in argument.as_ref().encode_wide() {
            if c == 0 {
                return Err(io::Error::other("NUL in clipboard worker argument"));
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
        return Err(io::Error::other("clipboard worker command line too long"));
    }
    Ok(out)
}

fn inherited(handle: HANDLE) -> io::Result<Handle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::other("clipboard pipe unavailable"));
    }
    let mut copy = ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle,
            GetCurrentProcess(),
            &mut copy,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(Handle(copy))
}

pub(super) fn run_if_system() -> io::Result<Option<i32>> {
    let mut raw = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let own = Handle(raw);
    if !token_is_system(own.0)? {
        return Ok(None);
    }
    let mut session = 0;
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } == 0 || session == 0 {
        return Err(io::Error::other(
            "clipboard requires an interactive session",
        ));
    }
    // Never redirect a secure-desktop clipboard request into the user's desktop.
    let desktop = unsafe { GetThreadDesktop(GetCurrentThreadId()) };
    let mut name = [0u16; 128];
    let mut size = 0;
    if unsafe {
        GetUserObjectInformationW(
            desktop,
            UOI_NAME,
            name.as_mut_ptr().cast(),
            std::mem::size_of_val(&name) as u32,
            &mut size,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    if !String::from_utf16_lossy(&name[..end]).eq_ignore_ascii_case("Default") {
        return Err(io::Error::other("clipboard unavailable on secure desktop"));
    }
    let mut raw = ptr::null_mut();
    if unsafe { WTSQueryUserToken(session, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let user = Handle(raw);
    let mut user_session = 0u32;
    let mut length = 0;
    if unsafe {
        GetTokenInformation(
            user.0,
            TokenSessionId,
            (&mut user_session as *mut u32).cast(),
            4,
            &mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if user_session != session || token_is_system(user.0)? {
        return Err(io::Error::other("clipboard user identity unavailable"));
    }
    let mut raw = ptr::null_mut();
    if unsafe { CreateEnvironmentBlock(&mut raw, user.0, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let environment = Environment(raw);
    let input = inherited(unsafe { GetStdHandle(STD_INPUT_HANDLE) })?;
    let output = inherited(unsafe { GetStdHandle(STD_OUTPUT_HANDLE) })?;
    let error = inherited(unsafe { GetStdHandle(STD_ERROR_HANDLE) })?;
    let handles = [input.0, output.0, error.0];
    let mut length = 0;
    unsafe {
        InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut length);
    }
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut attributes = vec![0usize; length.div_ceil(std::mem::size_of::<usize>())];
    if unsafe {
        InitializeProcThreadAttributeList(attributes.as_mut_ptr().cast(), 1, 0, &mut length)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut attributes = Attributes(attributes);
    if unsafe {
        UpdateProcThreadAttribute(
            attributes.pointer().cast(),
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            handles.as_ptr().cast(),
            std::mem::size_of_val(&handles),
            ptr::null_mut(),
            ptr::null(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let job = Handle(unsafe { CreateJobObjectW(ptr::null(), ptr::null()) });
    if job.0.is_null() {
        return Err(io::Error::last_os_error());
    }
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
    let executable = std::env::current_exe()?;
    let application: Vec<u16> = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut arguments = command_line(
        std::iter::once(executable.as_os_str().to_owned()).chain(std::env::args_os().skip(1)),
    )?;
    let mut desktop: Vec<u16> = "winsta0\\default\0".encode_utf16().collect();
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = std::mem::size_of_val(&startup) as u32;
    startup.StartupInfo.lpDesktop = desktop.as_mut_ptr();
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = input.0;
    startup.StartupInfo.hStdOutput = output.0;
    startup.StartupInfo.hStdError = error.0;
    startup.lpAttributeList = attributes.pointer().cast();
    let mut created: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // The child receives only the three private protocol handles, a fresh user
    // environment and that session's ordinary token. No impersonation/fallback.
    if unsafe {
        CreateProcessAsUserW(
            user.0,
            application.as_ptr(),
            arguments.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            CREATE_NO_WINDOW
                | CREATE_SUSPENDED
                | CREATE_UNICODE_ENVIRONMENT
                | EXTENDED_STARTUPINFO_PRESENT,
            environment.0,
            ptr::null(),
            &startup.StartupInfo,
            &mut created,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let process = Handle(created.hProcess);
    let thread = Handle(created.hThread);
    if unsafe { AssignProcessToJobObject(job.0, process.0) } == 0
        || unsafe { ResumeThread(thread.0) } == u32::MAX
    {
        let failure = io::Error::last_os_error();
        unsafe {
            TerminateProcess(process.0, 1);
            WaitForSingleObject(process.0, 5000);
        }
        return Err(failure);
    }
    // Closing the parent's job (including an abrupt kill_on_drop) also kills
    // the user child. Parent-owned pipe copies close when this worker exits.
    if unsafe { WaitForSingleObject(process.0, u32::MAX) } != WAIT_OBJECT_0 {
        return Err(io::Error::last_os_error());
    }
    let mut code = 1;
    if unsafe { GetExitCodeProcess(process.0, &mut code) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Some(code as i32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn CommandLineToArgvW(command: *const u16, count: *mut i32) -> *mut *mut u16;
    }

    #[test]
    fn worker_arguments_roundtrip_through_windows_parser() {
        let expected: Vec<OsString> = [
            "C:\\Program Files\\Pixels\\worker.exe",
            "",
            "clipboard",
            "C:\\trailing space \\",
            "quotes \\\"and\\\" 中文",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let line = command_line(&expected).unwrap();
        let mut count = 0;
        // SAFETY: line is NUL-terminated and count is writable. Windows owns
        // this allocation until LocalFree, after all returned strings are copied.
        let arguments = unsafe { CommandLineToArgvW(line.as_ptr(), &mut count) };
        assert!(!arguments.is_null());
        let actual = unsafe {
            std::slice::from_raw_parts(arguments, count as usize)
                .iter()
                .map(|argument| {
                    let mut size = 0;
                    while *argument.add(size) != 0 {
                        size += 1;
                    }
                    OsString::from_wide(std::slice::from_raw_parts(*argument, size))
                })
                .collect::<Vec<_>>()
        };
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(arguments.cast());
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn worker_arguments_reject_nul_and_windows_length_overflow() {
        let nul = OsString::from_wide(&[65, 0, 66]);
        assert!(command_line([nul]).is_err());
        assert!(command_line([OsString::from("x".repeat(32767))]).is_err());
    }
}
