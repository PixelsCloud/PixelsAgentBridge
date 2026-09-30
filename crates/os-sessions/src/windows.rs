use super::{SessionBatch, bounded};
use pab_protocol::OsSessionInfo;
use std::{ffi::c_void, ptr, slice};
use windows_sys::Win32::System::RemoteDesktop::{self as wts, *};

struct WtsBuffer(*mut c_void);
impl Drop for WtsBuffer {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: All instances own memory allocated by WTS, freed exactly once.
            unsafe { WTSFreeMemory(self.0) };
        }
    }
}
fn field(id: u32, kind: WTS_INFO_CLASS) -> Result<Vec<u16>, String> {
    let mut p = ptr::null_mut();
    let mut bytes = 0;
    // SAFETY: Local server handle and writable pointer/size outputs are valid.
    let success = unsafe {
        WTSQuerySessionInformationW(WTS_CURRENT_SERVER_HANDLE, id, kind, &mut p, &mut bytes)
    };
    let _buffer = WtsBuffer(p.cast());
    if success == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if bytes == 0 {
        return Ok(vec![]);
    }
    if p.is_null() {
        return Err("invalid WTS field buffer".into());
    }
    if bytes % 2 != 0 || bytes > 64 * 1024 {
        return Err("invalid WTS field size".into());
    }
    // SAFETY: WTS reports initialized byte count, checked for u16 alignment/size; owner stays alive.
    Ok(unsafe { slice::from_raw_parts(p, bytes as usize / 2) }.to_vec())
}
fn text_field(
    id: u32,
    kind: WTS_INFO_CLASS,
    label: &str,
    errors: &mut Vec<String>,
) -> Option<String> {
    match field(id, kind) {
        Ok(v) => {
            let n = v.iter().position(|c| *c == 0).unwrap_or(v.len());
            let s = String::from_utf16_lossy(&v[..n]);
            (!s.is_empty()).then(|| bounded(&s))
        }
        Err(e) => {
            errors.push(format!("{label}: {}", bounded(&e)));
            None
        }
    }
}
pub(super) fn collect() -> Result<SessionBatch, String> {
    let mut p = ptr::null_mut();
    let mut count = 0;
    // SAFETY: WTS writes the array pointer and length. Buffer ownership immediately follows.
    let success =
        unsafe { WTSEnumerateSessionsW(WTS_CURRENT_SERVER_HANDLE, 0, 1, &mut p, &mut count) };
    let _buffer = WtsBuffer(p.cast());
    if success == 0 {
        return Err(format!(
            "WTS enumerate: {}",
            std::io::Error::last_os_error()
        ));
    }
    if count > 0 && p.is_null() {
        return Err("invalid WTS session buffer".into());
    }
    let records = if count == 0 {
        &[]
    } else {
        // SAFETY: WTS returned count initialized entries; only inspect bounded prefix while owned.
        unsafe { slice::from_raw_parts(p, count.min(4096) as usize) }
    };
    let mut entries = Vec::new();
    for r in records {
        let state = match r.State {
            wts::WTSActive => "active",
            wts::WTSConnected => "connected",
            wts::WTSConnectQuery => "connect_query",
            wts::WTSShadow => "shadow",
            wts::WTSDisconnected => "disconnected",
            wts::WTSIdle => "idle",
            wts::WTSListen => "listen",
            wts::WTSReset => "reset",
            wts::WTSDown => "down",
            wts::WTSInit => "init",
            _ => "unknown",
        };
        let mut errors = vec![];
        let user_name = text_field(r.SessionId, WTSUserName, "user", &mut errors);
        let domain = text_field(r.SessionId, WTSDomainName, "domain", &mut errors);
        let terminal = text_field(r.SessionId, WTSWinStationName, "terminal", &mut errors);
        let client_name = text_field(r.SessionId, WTSClientName, "client_name", &mut errors);
        let protocol = match field(r.SessionId, WTSClientProtocolType) {
            Ok(v) if !v.is_empty() => Some(v[0]),
            Ok(_) => None,
            Err(e) => {
                errors.push(format!("protocol: {}", bounded(&e)));
                None
            }
        };
        entries.push(OsSessionInfo {
            id: r.SessionId.to_string(),
            user_name,
            user_id: None,
            domain,
            state: state.into(),
            active: (state != "unknown").then_some(r.State == WTSActive),
            remote: protocol.and_then(|p| (p <= 2).then_some(p != 0)),
            seat: None,
            terminal,
            client_name,
            session_type: protocol.map(|p| match p {
                0 => "console".into(),
                2 => "rdp".into(),
                v => format!("protocol_{v}"),
            }),
            errors,
        });
    }
    entries.sort_by_key(|s| s.id.parse::<u32>().unwrap_or_default());
    Ok(SessionBatch {
        backend: "windows_wts",
        entries,
        truncated: count > 4096,
    })
}
