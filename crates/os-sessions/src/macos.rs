//! utmpx records represent logins, not accounts or PAB terminal sessions.
use super::*;
use std::{ffi::CString, os::unix::fs::MetadataExt, sync::Mutex};
static UTMP: Mutex<()> = Mutex::new(());

fn field(bytes: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = bytes
        .iter()
        .take_while(|b| **b != 0)
        .map(|b| *b as u8)
        .collect();
    bounded(&String::from_utf8_lossy(&bytes))
}

fn uid(name: &str) -> Option<u32> {
    let name = CString::new(name).ok()?;
    let mut record = unsafe { std::mem::zeroed::<libc::passwd>() };
    let mut found = std::ptr::null_mut();
    let mut buffer = vec![0u8; 65536];
    // SAFETY: reentrant lookup uses caller-owned storage, only read after success.
    let status = unsafe {
        libc::getpwnam_r(
            name.as_ptr(),
            &mut record,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut found,
        )
    };
    (status == 0 && !found.is_null()).then_some(record.pw_uid)
}

pub(super) fn collect() -> Result<SessionBatch, String> {
    let _lock = UTMP.lock().map_err(|_| "session reader lock unavailable")?;
    let console_uid = std::fs::metadata("/dev/console").ok().map(|m| m.uid());
    let mut entries = Vec::new();
    let mut truncated = false;
    // SAFETY: serialized access to libc's global utmpx iterator; each record is copied
    // before calling getutxent again and endutxent closes the iterator before return.
    unsafe {
        libc::setutxent();
        loop {
            let ptr = libc::getutxent();
            if ptr.is_null() {
                break;
            }
            let record = *ptr;
            if record.ut_type != libc::USER_PROCESS {
                continue;
            }
            if entries.len() == 4096 {
                truncated = true;
                break;
            }
            let user = field(&record.ut_user);
            let tty = field(&record.ut_line);
            let host = field(&record.ut_host);
            let user_id = uid(&user);
            let console = tty == "console";
            let active = console && user_id.is_some() && user_id == console_uid;
            entries.push(OsSessionInfo {
                id: format!("utmpx:{}:{}:{}", record.ut_pid, record.ut_tv.tv_sec, tty),
                user_name: Some(user),
                user_id,
                domain: None,
                state: if active { "active" } else { "logged_in" }.into(),
                active: console.then_some(active),
                remote: Some(!host.is_empty()),
                seat: console.then(|| "console".into()),
                terminal: Some(tty),
                client_name: (!host.is_empty()).then_some(host),
                session_type: Some(if console { "aqua" } else { "tty" }.into()),
                errors: vec![],
            });
        }
        libc::endutxent();
    }
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(SessionBatch {
        backend: "macos_utmpx",
        entries,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fields_are_bounded_and_nul_terminated() {
        assert_eq!(field(&[97, 0, 98]), "a");
    }
    #[test]
    fn native_sessions_are_bounded() {
        let batch = collect().unwrap();
        assert_eq!(batch.backend, "macos_utmpx");
        assert!(batch.entries.len() <= 4096);
    }
}
