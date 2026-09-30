use super::*;
use std::{
    io, ptr,
    time::{Duration, Instant},
};
use windows_service::{
    service::{ServiceAccess, ServiceExitCode, ServiceState},
    service_manager::{ServiceManager, ServiceManagerAccess},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Services::*, Threading::*},
    UI::WindowsAndMessaging::*,
};

struct ProcessHandle(HANDLE);
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn open(pid: u32, terminate: bool) -> Result<ProcessHandle, String> {
    let access = PROCESS_QUERY_LIMITED_INFORMATION
        | PROCESS_SYNCHRONIZE
        | if terminate { PROCESS_TERMINATE } else { 0 };
    let h = unsafe { OpenProcess(access, 0, pid) };
    if h.is_null() {
        Err(io::Error::last_os_error().to_string())
    } else {
        Ok(ProcessHandle(h))
    }
}
fn creation(h: &ProcessHandle) -> Result<String, String> {
    let mut c = FILETIME::default();
    let mut e = FILETIME::default();
    let mut k = FILETIME::default();
    let mut u = FILETIME::default();
    if unsafe { GetProcessTimes(h.0, &mut c, &mut e, &mut k, &mut u) } == 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    Ok(format!(
        "windows_filetime:{}",
        ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64
    ))
}
fn exited(h: &ProcessHandle) -> Result<bool, String> {
    match unsafe { WaitForSingleObject(h.0, 0) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(io::Error::last_os_error().to_string()),
    }
}
pub(super) fn identity(pid: u32) -> Result<String, String> {
    let h = open(pid, false)?;
    if exited(&h)? {
        return Err("process already exited".into());
    }
    creation(&h)
}
struct CloseWindows {
    pid: u32,
    sent: u32,
    failed: u32,
}
unsafe extern "system" fn close_window(hwnd: HWND, param: LPARAM) -> i32 {
    // EnumWindows invokes this callback synchronously while the stack context is alive.
    let context = unsafe { &mut *(param as *mut CloseWindows) };
    let mut owner = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut owner);
    }
    if owner == context.pid {
        if unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) } != 0 {
            context.sent += 1;
        } else {
            context.failed += 1;
        }
    }
    1
}
fn terminate(
    pid: u32,
    expected: &str,
    timeout_ms: u32,
    force: bool,
) -> Result<SystemQueryData, String> {
    if pid == std::process::id() || pid <= 4 {
        return Err("protected PID: cannot terminate Executor or system process".into());
    }
    let h = open(pid, force)?;
    let actual = creation(&h)?;
    if actual != expected {
        return Err("process_identity_mismatch: PID now belongs to a different process".into());
    }
    let mut r = ProcessTerminationResult {
        pid,
        identity: actual,
        outcome: "completed".into(),
        method: "already_exited".into(),
        graceful_supported: false,
        forced: false,
        error: None,
    };
    if exited(&h)? {
        return Ok(SystemQueryData::ProcessTermination { result: r });
    }
    let mut context = CloseWindows {
        pid,
        sent: 0,
        failed: 0,
    };
    if unsafe { EnumWindows(Some(close_window), &mut context as *mut _ as LPARAM) } == 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    r.graceful_supported = context.sent > 0;
    r.method = if r.graceful_supported {
        "wm_close"
    } else {
        "no_closeable_window"
    }
    .into();
    if context.failed > 0 {
        r.error = Some("one or more WM_CLOSE messages could not be sent".into());
    }
    let wait = if r.graceful_supported {
        unsafe { WaitForSingleObject(h.0, timeout_ms) }
    } else {
        WAIT_TIMEOUT
    };
    match wait {
        WAIT_OBJECT_0 => {
            r.error = None;
        }
        WAIT_TIMEOUT if force => {
            r.forced = true;
            r.method = "terminate_process".into();
            if unsafe { TerminateProcess(h.0, 1) } == 0 && !exited(&h)? {
                r.outcome = "failed".into();
                r.error = Some(io::Error::last_os_error().to_string());
            } else if unsafe { WaitForSingleObject(h.0, 5000) } != WAIT_OBJECT_0 {
                r.outcome = "timeout".into();
                r.error = Some(
                    "termination requested; process exit not confirmed within 5 seconds".into(),
                );
            } else {
                r.error = None;
            }
        }
        WAIT_TIMEOUT => {
            r.outcome = if r.graceful_supported {
                "timeout"
            } else {
                "unsupported_graceful"
            }
            .into();
            r.error=Some(if r.graceful_supported {"WM_CLOSE sent but process did not exit before deadline"} else {"no closeable top-level window; use service control for services, or force=true for explicit termination"}.into());
        }
        _ => {
            r.outcome = "failed".into();
            r.error = Some(io::Error::last_os_error().to_string());
        }
    }
    Ok(SystemQueryData::ProcessTermination { result: r })
}
fn state(s: ServiceState) -> &'static str {
    match s {
        ServiceState::Stopped => "stopped",
        ServiceState::StartPending => "start_pending",
        ServiceState::StopPending => "stop_pending",
        ServiceState::Running => "running",
        ServiceState::ContinuePending => "continue_pending",
        ServiceState::PausePending => "pause_pending",
        ServiceState::Paused => "paused",
    }
}
fn manager() -> Result<ServiceManager, String> {
    ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| e.to_string())
}
fn read(name: &str, service: &windows_service::service::Service) -> Result<ServiceInfo, String> {
    let status = service.query_status().map_err(|e| e.to_string())?;
    let mut r = empty_service("windows_scm", name);
    r.state = state(status.current_state).into();
    r.pid = status.process_id.filter(|p| *p > 0);
    r.exit_code = Some(match status.exit_code {
        ServiceExitCode::Win32(c) | ServiceExitCode::ServiceSpecific(c) => c as i64,
    });
    r.checkpoint = Some(status.checkpoint);
    r.wait_hint_ms = Some(status.wait_hint.as_millis() as u64);
    match service.query_config() {
        Ok(c) => {
            r.display_name = Some(bounded(&c.display_name.to_string_lossy(), 512));
            r.executable = executable_from_config(&c.executable_path)
                .map_err(|e| r.errors.push(e))
                .ok();
            r.account = c.account_name.map(|a| bounded(&a.to_string_lossy(), 256));
            r.start_mode = Some(format!("{:?}", c.start_type).to_lowercase());
        }
        Err(e) => r.errors.push(bounded(&e.to_string(), 512)),
    }
    Ok(r)
}
fn executable_from_config(path: &std::path::Path) -> Result<String, String> {
    use std::os::windows::ffi::OsStrExt;
    let line: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut argc = 0;
    let argv =
        unsafe { windows_sys::Win32::UI::Shell::CommandLineToArgvW(line.as_ptr(), &mut argc) };
    if argv.is_null() {
        return Err(bounded(&io::Error::last_os_error().to_string(), 256));
    }
    // CommandLineToArgvW returns one LocalAlloc buffer; all argv strings remain valid until LocalFree.
    let result = if argc > 0 {
        let first = unsafe { *argv };
        let mut len = 0;
        while unsafe { *first.add(len) } != 0 {
            len += 1;
        }
        Ok(bounded(
            &String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(first, len) }),
            4096,
        ))
    } else {
        Err("configured executable could not be parsed".into())
    };
    unsafe {
        LocalFree(argv.cast());
    }
    result
}
fn get(name: &str) -> Result<ServiceInfo, String> {
    let m = manager()?;
    let service = m
        .open_service(
            name,
            ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG,
        )
        .or_else(|_| m.open_service(name, ServiceAccess::QUERY_STATUS))
        .map_err(|e| e.to_string())?;
    read(name, &service)
}
struct Scm(SC_HANDLE);
impl Drop for Scm {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}
fn text_in_buffer(p: *const u16, base: usize, end: usize) -> Result<String, String> {
    let address = p as usize;
    if address < base || address >= end || !address.is_multiple_of(2) {
        return Err("SCM string outside initialized buffer".into());
    }
    // Windows points strings inside the successful API result buffer. Validate bounds/alignment before reading.
    let words = unsafe { std::slice::from_raw_parts(p, (end - address) / 2) };
    let length = words
        .iter()
        .position(|v| *v == 0)
        .ok_or("unterminated SCM string")?;
    Ok(bounded(&String::from_utf16_lossy(&words[..length]), 512))
}
fn list() -> Result<Vec<ServiceInfo>, String> {
    let h = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_ENUMERATE_SERVICE) };
    if h.is_null() {
        return Err(io::Error::last_os_error().to_string());
    }
    let h = Scm(h);
    let mut needed = 0;
    let mut count = 0;
    // Retry inventory growth with a bounded buffer; never use an ERROR_MORE_DATA partial buffer.
    let mut storage = vec![0usize; 32768];
    for _ in 0..4 {
        let bytes = storage.len() * std::mem::size_of::<usize>();
        if unsafe {
            EnumServicesStatusExW(
                h.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                storage.as_mut_ptr().cast(),
                bytes as u32,
                &mut needed,
                &mut count,
                ptr::null_mut(),
                ptr::null(),
            )
        } != 0
        {
            let used = (count as usize)
                .checked_mul(std::mem::size_of::<ENUM_SERVICE_STATUS_PROCESSW>())
                .ok_or("SCM inventory overflow")?;
            if used > bytes {
                return Err("SCM inventory exceeds buffer".into());
            }
            let rows = unsafe {
                std::slice::from_raw_parts(
                    storage.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>(),
                    count as usize,
                )
            };
            let base = storage.as_ptr() as usize;
            let mut entries = vec![];
            for row in rows {
                let name = text_in_buffer(row.lpServiceName, base, base + bytes)?;
                let mut r = empty_service("windows_scm", &name);
                r.display_name = Some(text_in_buffer(row.lpDisplayName, base, base + bytes)?);
                r.state = match row.ServiceStatusProcess.dwCurrentState {
                    SERVICE_STOPPED => "stopped",
                    SERVICE_START_PENDING => "start_pending",
                    SERVICE_STOP_PENDING => "stop_pending",
                    SERVICE_RUNNING => "running",
                    SERVICE_CONTINUE_PENDING => "continue_pending",
                    SERVICE_PAUSE_PENDING => "pause_pending",
                    SERVICE_PAUSED => "paused",
                    _ => "unknown",
                }
                .into();
                r.pid = (row.ServiceStatusProcess.dwProcessId > 0)
                    .then_some(row.ServiceStatusProcess.dwProcessId);
                r.exit_code = Some(row.ServiceStatusProcess.dwWin32ExitCode as i64);
                r.checkpoint = Some(row.ServiceStatusProcess.dwCheckPoint);
                r.wait_hint_ms = Some(row.ServiceStatusProcess.dwWaitHint as u64);
                entries.push(r);
            }
            return Ok(entries);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_MORE_DATA as i32) {
            return Err(error.to_string());
        }
        let requested = (needed as usize).max(bytes * 2);
        if requested > 2 * 1024 * 1024 {
            return Err("SCM inventory exceeds 2 MiB buffer limit".into());
        }
        storage.resize(requested.div_ceil(std::mem::size_of::<usize>()), 0);
    }
    Err("SCM inventory keeps changing; retry with a new request ID".into())
}
fn wait(
    service: &windows_service::service::Service,
    target: ServiceState,
    deadline: Instant,
) -> Result<(), String> {
    loop {
        let status = service.query_status().map_err(|e| e.to_string())?;
        if status.current_state == target {
            return Ok(());
        }
        if target == ServiceState::Running && status.current_state == ServiceState::Stopped {
            return Err(format!(
                "service stopped before running: {:?}",
                status.exit_code
            ));
        }
        if Instant::now() >= deadline {
            return Err("service state wait timed out; accepted action is not rolled back".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn control(
    name: &str,
    action: ServiceControlAction,
    timeout_ms: u32,
) -> Result<SystemQueryData, String> {
    let m = manager()?;
    let access = ServiceAccess::QUERY_STATUS
        | ServiceAccess::QUERY_CONFIG
        | match action {
            ServiceControlAction::Start => ServiceAccess::START,
            ServiceControlAction::Stop => ServiceAccess::STOP,
            ServiceControlAction::Restart => ServiceAccess::START | ServiceAccess::STOP,
            ServiceControlAction::Enable | ServiceControlAction::Disable => {
                ServiceAccess::CHANGE_CONFIG
            }
        };
    let service = m.open_service(name, access).map_err(|e| e.to_string())?;
    let status = service.query_status().map_err(|e| e.to_string())?;
    if status.process_id == Some(std::process::id())
        && matches!(
            action,
            ServiceControlAction::Stop | ServiceControlAction::Restart
        )
    {
        return Err("cannot stop or restart Executor's own service".into());
    }
    let mut r = control_result(name, action);
    r.service = Some(read(name, &service)?);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
    let result = (|| -> Result<(), String> {
        if matches!(
            action,
            ServiceControlAction::Stop | ServiceControlAction::Restart
        ) && status.current_state != ServiceState::Stopped
        {
            r.phase = "stopping".into();
            if status.current_state != ServiceState::StopPending {
                service.stop().map_err(|e| e.to_string())?;
                r.changed = true;
            }
            wait(&service, ServiceState::Stopped, deadline)?;
        }
        if matches!(
            action,
            ServiceControlAction::Start | ServiceControlAction::Restart
        ) {
            let current = service.query_status().map_err(|e| e.to_string())?;
            if current.current_state != ServiceState::Running {
                r.phase = "starting".into();
                if Instant::now() >= deadline {
                    return Err("service state wait timed out before start; stopped service is not rolled back".into());
                }
                if current.current_state != ServiceState::StartPending {
                    service.start::<&str>(&[]).map_err(|e| e.to_string())?;
                    r.changed = true;
                }
                wait(&service, ServiceState::Running, deadline)?;
            }
        }
        if matches!(
            action,
            ServiceControlAction::Enable | ServiceControlAction::Disable
        ) {
            r.phase = "configuration".into();
            let target = if action == ServiceControlAction::Enable {
                SERVICE_AUTO_START
            } else {
                SERVICE_DISABLED
            };
            if service
                .query_config()
                .map_err(|e| e.to_string())?
                .start_type
                .to_raw()
                == target
            {
                return Ok(());
            }
            // windows-service lacks a start-type-only setter. Keep all other configuration fields unchanged.
            if unsafe {
                ChangeServiceConfigW(
                    service.raw_handle(),
                    SERVICE_NO_CHANGE,
                    target,
                    SERVICE_NO_CHANGE,
                    ptr::null(),
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error().to_string());
            }
            r.changed = true;
            let config = service.query_config().map_err(|e| e.to_string())?;
            if config.start_type.to_raw() != target {
                return Err("startup configuration verification failed".into());
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => r.phase = "verified".into(),
        Err(e) => {
            r.outcome = if e.contains("timed out") {
                "timeout"
            } else {
                "failed"
            }
            .into();
            r.error = Some(bounded(&e, 1024));
        }
    }
    match read(name, &service) {
        Ok(s) => r.service = Some(s),
        Err(e) => {
            r.error = Some(bounded(&e, 1024));
            r.outcome = "unconfirmed".into();
        }
    }
    Ok(SystemQueryData::ServiceControl { result: r })
}
pub(super) fn execute(query: &SystemQuery) -> Result<SystemQueryData, String> {
    match query {
        SystemQuery::TerminateProcess {
            pid,
            identity,
            timeout_ms,
            force,
        } => terminate(*pid, identity, *timeout_ms, *force),
        SystemQuery::Services { .. } => Ok(SystemQueryData::Services {
            backend: "windows_scm".into(),
            entries: list()?,
        }),
        SystemQuery::Service { name } => Ok(SystemQueryData::Service {
            service: get(name)?,
        }),
        SystemQuery::ServiceControl {
            name,
            control: action,
            timeout_ms,
        } => control(name, *action, *timeout_ms),
        _ => Err("unsupported lifecycle operation".into()),
    }
}
