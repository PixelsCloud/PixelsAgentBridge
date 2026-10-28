//! Cooperative process-lifetime reservations within one Linux network namespace.
//! Abstract sockets need no writable lock directory and disappear on owner death.
//! These are not distributed locks for shared volumes or other network namespaces.
use std::{
    ffi::OsStr,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
};

pub struct ProcessFileLock {
    _socket: OwnedFd,
}

impl ProcessFileLock {
    pub fn try_acquire(digest: &str) -> io::Result<Option<Self>> {
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid file reservation key",
            ));
        }
        let name = format!("PixelsAgentBridge.FileTarget.v1.{digest}");
        // sockaddr_un is a plain C structure. Its leading NUL selects the
        // abstract namespace; the supplied length excludes any trailing NUL.
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        if name.len() + 1 > address.sun_path.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "reservation name too long"));
        }
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (destination, source) in address.sun_path[1..].iter_mut().zip(name.bytes()) {
            *destination = source as libc::c_char;
        }
        // CLOEXEC prevents child programs from accidentally extending ownership.
        let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let socket = unsafe { OwnedFd::from_raw_fd(raw) };
        let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + 1 + name.len();
        let result = unsafe {
            libc::bind(
                socket.as_raw_fd(),
                std::ptr::from_ref(&address).cast(),
                length as libc::socklen_t,
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EADDRINUSE) {
                return Ok(None);
            }
            return Err(error);
        }
        Ok(Some(Self { _socket: socket }))
    }
}

/// Linux names are opaque native bytes, including non-UTF-8 filenames.
pub fn fold_file_name(name: &OsStr) -> Vec<u8> {
    name.as_bytes().to_vec()
}
