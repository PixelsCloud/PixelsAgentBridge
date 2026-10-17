//! Read-only logind observations for the media route. No remote authorization,
//! session creation, identity changes or interpretation of user environment.
use libloading::Library;
use std::{
    ffi::{CStr, CString, c_char, c_int},
    io,
    sync::OnceLock,
};

type PidSession = unsafe extern "C" fn(libc::pid_t, *mut *mut c_char) -> c_int;
type UidSession = unsafe extern "C" fn(libc::uid_t, *mut *mut c_char) -> c_int;
type SessionTest = unsafe extern "C" fn(*const c_char) -> c_int;
type SessionString = unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> c_int;
type SessionUid = unsafe extern "C" fn(*const c_char, *mut libc::uid_t) -> c_int;
struct Api {
    _library: Library,
    pid_session: PidSession,
    uid_session: UidSession,
    active: SessionTest,
    remote: SessionTest,
    kind: SessionString,
    class: SessionString,
    seat: SessionString,
    uid: SessionUid,
}
impl Api {
    fn load() -> Result<Self, String> {
        // Versioned runtime dependency, as in the existing journal adapter. A
        // headless/non-systemd install keeps working without development files.
        let library = unsafe { Library::new("libsystemd.so.0") }.map_err(|e| e.to_string())?;
        // SAFETY: signatures are the public sd-login ABI; the library is kept
        // alive with the function pointers for the lifetime of this Api.
        unsafe {
            macro_rules! symbol {
                ($name:literal, $ty:ty) => {
                    *library
                        .get::<$ty>(concat!($name, "\0").as_bytes())
                        .map_err(|e| e.to_string())?
                };
            }
            Ok(Self {
                pid_session: symbol!("sd_pid_get_session", PidSession),
                uid_session: symbol!("sd_uid_get_display", UidSession),
                active: symbol!("sd_session_is_active", SessionTest),
                remote: symbol!("sd_session_is_remote", SessionTest),
                kind: symbol!("sd_session_get_type", SessionString),
                class: symbol!("sd_session_get_class", SessionString),
                seat: symbol!("sd_session_get_seat", SessionString),
                uid: symbol!("sd_session_get_uid", SessionUid),
                _library: library,
            })
        }
    }
    fn get() -> io::Result<&'static Self> {
        static API: OnceLock<Result<Api, String>> = OnceLock::new();
        API.get_or_init(Self::load)
            .as_ref()
            .map_err(|e| io::Error::other(format!("logind observation unavailable: {e}")))
    }
}
fn string(call: impl FnOnce(*mut *mut c_char) -> c_int) -> io::Result<CString> {
    let mut pointer = std::ptr::null_mut();
    let status = call(&mut pointer);
    struct Owned(*mut c_char);
    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: sd-login returns malloc-owned strings; free(NULL) is valid.
            unsafe { libc::free(self.0.cast()) };
        }
    }
    let owned = Owned(pointer);
    if status < 0 {
        return Err(io::Error::from_raw_os_error(-status));
    }
    if owned.0.is_null() {
        return Err(io::Error::other("missing logind session field"));
    }
    // SAFETY: successful sd-login string outputs are NUL-terminated.
    let value = unsafe { CStr::from_ptr(owned.0) };
    if value.to_bytes().is_empty() || value.to_bytes().len() > 1024 {
        return Err(io::Error::other("invalid logind session field"));
    }
    Ok(value.to_owned())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinuxGraphicalSession {
    id: CString,
    uid: u32,
    kind: CString,
    seat: CString,
    class: CString,
}
impl LinuxGraphicalSession {
    /// The UID must come from process_user_identity, never from registration or
    /// XDG_SESSION_ID. User-manager applications can live outside session.scope;
    /// only ENODATA permits fallback to logind's primary graphical user session.
    pub fn for_process(pid: u32, uid: u32) -> io::Result<Self> {
        Self::observe(pid, uid, false)
    }
    /// Media may belong to the active local login screen. This does not expose
    /// the greeter as an application/command execution user or grant access to
    /// another session. UID/PID and logind facts remain independently observed.
    pub fn for_media_process(pid: u32, uid: u32) -> io::Result<Self> {
        Self::observe(pid, uid, true)
    }
    fn observe(pid: u32, uid: u32, media: bool) -> io::Result<Self> {
        let pid = libc::pid_t::try_from(pid)
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(|| io::Error::other("invalid graphical process"))?;
        if uid == 0 {
            return Err(io::Error::other("user graphical session required"));
        }
        let api = Api::get()?;
        // SAFETY: valid process/UID values and writable output pointer. Strings
        // returned below are owned and remain live through each sd-login call.
        let id = match string(|out| unsafe { (api.pid_session)(pid, out) }) {
            Ok(id) => id,
            Err(e) if e.raw_os_error() == Some(libc::ENODATA) => {
                string(|out| unsafe { (api.uid_session)(uid, out) })?
            }
            Err(e) => return Err(e),
        };
        let mut observed_uid = u32::MAX;
        let status = unsafe { (api.uid)(id.as_ptr(), &mut observed_uid) };
        if status < 0 {
            return Err(io::Error::from_raw_os_error(-status));
        }
        let active = unsafe { (api.active)(id.as_ptr()) };
        let remote = unsafe { (api.remote)(id.as_ptr()) };
        let kind = string(|out| unsafe { (api.kind)(id.as_ptr(), out) })?;
        let class = string(|out| unsafe { (api.class)(id.as_ptr(), out) })?;
        let seat = string(|out| unsafe { (api.seat)(id.as_ptr(), out) })?;
        if !eligible(
            uid,
            observed_uid,
            active,
            remote,
            kind.as_bytes(),
            class.as_bytes(),
            media,
        ) {
            return Err(io::Error::other(
                "process is outside the active local graphical user session",
            ));
        }
        Ok(Self {
            id,
            uid,
            kind,
            seat,
            class,
        })
    }
    pub fn current(&self, pid: u32) -> bool {
        Self::observe(pid, self.uid, self.class.as_bytes() == b"greeter").as_ref().ok()
            == Some(self)
    }
}
fn eligible(
    expected_uid: u32,
    uid: u32,
    active: c_int,
    remote: c_int,
    kind: &[u8],
    class: &[u8],
    media: bool,
) -> bool {
    expected_uid != 0
        && uid == expected_uid
        && active > 0
        && remote == 0
        && matches!(kind, b"x11" | b"wayland")
        && (class == b"user" || (media && class == b"greeter"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn graphical_observation_requires_live_matching_local_user_session() {
        for kind in [b"x11".as_slice(), b"wayland"] {
            assert!(eligible(1000, 1000, 1, 0, kind, b"user", false));
        }
        for (uid, active, remote, kind, class) in [
            (1001, 1, 0, b"wayland".as_slice(), b"user".as_slice()),
            (1000, 0, 0, b"wayland", b"user"),
            (1000, -1, 0, b"wayland", b"user"),
            (1000, 1, 1, b"wayland", b"user"),
            (1000, 1, -1, b"wayland", b"user"),
            (1000, 1, 0, b"tty", b"user"),
            (1000, 1, 0, b"wayland", b"greeter"),
        ] {
            assert!(!eligible(1000, uid, active, remote, kind, class, false));
        }
        assert!(!eligible(0, 0, 1, 0, b"wayland", b"user", false));
    }
    #[test]
    fn login_screen_is_media_only_and_still_requires_active_matching_local_identity() {
        for kind in [b"x11".as_slice(), b"wayland"] {
            assert!(eligible(128, 128, 1, 0, kind, b"greeter", true));
            assert!(!eligible(128, 128, 1, 0, kind, b"greeter", false));
        }
        for (uid, active, remote, kind, class) in [
            (1000, 1, 0, b"x11".as_slice(), b"greeter".as_slice()),
            (128, 0, 0, b"x11", b"greeter"),
            (128, -1, 0, b"x11", b"greeter"),
            (128, 1, 1, b"x11", b"greeter"),
            (128, 1, -1, b"x11", b"greeter"),
            (128, 1, 0, b"tty", b"greeter"),
            (128, 1, 0, b"x11", b"background"),
        ] {
            assert!(!eligible(128, uid, active, remote, kind, class, true));
        }
        assert!(!eligible(0, 0, 1, 0, b"x11", b"greeter", true));
    }
}
