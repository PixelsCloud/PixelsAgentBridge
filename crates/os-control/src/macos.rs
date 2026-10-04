//! Darwin process identity uses an audit token (including PID generation).
//! Signals are delivered with that token by the kernel, never by a bare PID.
use super::*;
use std::time::{Duration, Instant};
mod services;

unsafe extern "C" {
    static mach_task_self_: u32;
    fn task_name_for_pid(task: u32, pid: i32, name: *mut u32) -> i32;
    fn task_info(task: u32, flavor: i32, info: *mut u32, count: *mut u32) -> i32;
    fn mach_port_deallocate(task: u32, name: u32) -> i32;
    fn proc_signal_with_audittoken(token: *const [u32; 8], signal: i32) -> i32;
}

fn audit_token(pid: u32) -> Result<[u32; 8], String> {
    let pid = i32::try_from(pid).map_err(|_| "invalid PID")?;
    let mut port = 0;
    let mut token = [0; 8];
    let mut count = 8;
    // SAFETY: all out-parameters are initialized, correctly sized writable storage.
    // The acquired Mach send right is released on every path after acquisition.
    unsafe {
        let status = task_name_for_pid(mach_task_self_, pid, &mut port);
        if status != 0 {
            return Err(format!(
                "process identity unavailable or permission denied (Mach {status})"
            ));
        }
        let status = task_info(port, 15, token.as_mut_ptr(), &mut count);
        mach_port_deallocate(mach_task_self_, port);
        if status != 0 || count != 8 || token[5] != pid as u32 {
            return Err(format!("process audit token unavailable (Mach {status})"));
        }
    }
    Ok(token)
}

fn token_string(token: &[u32; 8]) -> String {
    format!(
        "macos_audit:{}",
        token.iter().map(|v| format!("{v:08x}")).collect::<String>()
    )
}
pub(super) fn identity(pid: u32) -> Result<String, String> {
    audit_token(pid).map(|t| token_string(&t))
}

fn signal(token: &[u32; 8], number: i32) -> Result<(), String> {
    // SAFETY: the kernel copies a complete audit_token_t; libproc returns errno directly.
    let error = unsafe { proc_signal_with_audittoken(token, number) };
    if error == 0 {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(error).to_string())
    }
}

fn exit_queue(pid: u32) -> Result<std::os::fd::OwnedFd, String> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let raw = unsafe { libc::kqueue() };
    if raw < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: kqueue returned a new owned descriptor; OwnedFd always closes it.
    let queue = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) };
    let event = libc::kevent {
        ident: pid as usize,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    if unsafe {
        libc::kevent(
            queue.as_raw_fd(),
            &event,
            1,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
        )
    } < 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(queue)
}

fn wait_exit(queue: &std::os::fd::OwnedFd, timeout: Duration) -> Result<bool, String> {
    use std::os::fd::AsRawFd;
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout = libc::timespec {
            tv_sec: remaining.as_secs() as i64,
            tv_nsec: remaining.subsec_nanos() as i64,
        };
        let mut event = unsafe { std::mem::zeroed::<libc::kevent>() };
        let count = unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                std::ptr::null(),
                0,
                &mut event,
                1,
                &timeout,
            )
        };
        if count < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.to_string());
        }
        if count == 0 {
            return Ok(false);
        }
        if event.flags & libc::EV_ERROR != 0 {
            return Err(std::io::Error::from_raw_os_error(event.data as i32).to_string());
        }
        if event.fflags & libc::NOTE_EXIT != 0 {
            return Ok(true);
        }
    }
}

pub(super) fn execute(query: &SystemQuery) -> Result<SystemQueryData, String> {
    let SystemQuery::TerminateProcess {
        pid,
        identity: expected,
        timeout_ms,
        force,
    } = query
    else {
        return services::execute(query);
    };
    if *pid <= 1 || *pid == std::process::id() {
        return Err("protected PID".into());
    }
    let token = audit_token(*pid)?;
    if token_string(&token) != *expected {
        return Err("process_identity_mismatch: query get_process again".into());
    }
    // Register exit observation before sending a signal, then recheck the audit
    // token so a reused PID cannot bind the observer to a different process.
    let queue = exit_queue(*pid)?;
    if audit_token(*pid)? != token {
        return Err("process identity changed before termination".into());
    }
    let mut result = ProcessTerminationResult {
        pid: *pid,
        identity: expected.clone(),
        outcome: "completed".into(),
        method: "sigterm_audit_token".into(),
        graceful_supported: true,
        forced: false,
        error: None,
    };
    let outcome = (|| {
        signal(&token, 15)?;
        if wait_exit(&queue, Duration::from_millis(u64::from(*timeout_ms)))? {
            return Ok(());
        }
        if *force {
            result.forced = true;
            result.method = "sigkill_audit_token".into();
            signal(&token, 9)?;
            if wait_exit(&queue, Duration::from_secs(5))? {
                return Ok(());
            }
        }
        result.outcome = "timeout".into();
        Err("signal sent; process exit not observed before deadline".into())
    })();
    if let Err(error) = outcome {
        if result.outcome == "completed" {
            result.outcome = "failed".into();
        }
        result.error = Some(error);
    }
    Ok(SystemQueryData::ProcessTermination { result })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audit_identity_is_stable_and_rejects_wrong_target() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let id = identity(child.id()).unwrap();
        assert_eq!(id, identity(child.id()).unwrap());
        let query = SystemQuery::TerminateProcess {
            pid: child.id(),
            identity: "wrong".into(),
            timeout_ms: 100,
            force: false,
        };
        assert!(execute(&query).unwrap_err().contains("identity_mismatch"));
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }
    #[test]
    fn terminates_only_test_child() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let query = SystemQuery::TerminateProcess {
            pid: child.id(),
            identity: identity(child.id()).unwrap(),
            timeout_ms: 2000,
            force: false,
        };
        // Reap concurrently: a zombie still has an observable PID until its parent waits.
        let waiter = std::thread::spawn(move || child.wait().unwrap());
        let SystemQueryData::ProcessTermination { result } = execute(&query).unwrap() else {
            panic!()
        };
        assert_eq!(result.outcome, "completed", "{result:?}");
        assert!(!result.forced);
        waiter.join().unwrap();
    }
}
