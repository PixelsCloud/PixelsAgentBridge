//! A nonblocking, process-lifetime reservation in the machine-wide namespace.
//! Only creation owns the reservation. Opening an existing object never does.
//! We do not wait on/release semaphore counts, which would survive owner death,
//! or acquire a thread-owned mutex that cannot safely move between async tasks.
use std::{
    ffi::OsStr,
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{FromRawHandle, OwnedHandle},
    },
};
use windows_sys::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, GetLastError, SetLastError},
    System::Threading::CreateSemaphoreExW,
};

pub struct ProcessFileLock {
    _handle: OwnedHandle,
}
impl ProcessFileLock {
    /// `digest` must identify a physical parent/name or an already opened object.
    /// No paths or secrets are exposed in the kernel object name. Default ACLs
    /// remain intact: access denied is an error, never a reason to skip locking.
    pub fn try_acquire(digest: &str) -> io::Result<Option<Self>> {
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid file reservation key",
            ));
        }
        let name: Vec<u16> = format!("Global\\PixelsAgentBridge.FileTarget.v1.{digest}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // No inheritance, no pointers retained by Windows, and the resulting
        // handle is immediately owned. SYNCHRONIZE is sufficient; no mutation
        // rights or elevated privileges are requested for existing objects.
        let (handle, error) = unsafe {
            SetLastError(0);
            let handle = CreateSemaphoreExW(std::ptr::null(), 0, 1, name.as_ptr(), 0, 0x0010_0000);
            (handle, GetLastError())
        };
        if handle.is_null() {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        if error == ERROR_ALREADY_EXISTS {
            return Ok(None);
        }
        Ok(Some(Self { _handle: handle }))
    }
}

/// Use the native one-code-unit uppercase mapping, preserving unpaired UTF-16.
/// Case-sensitive directories may be conservatively over-reserved.
pub fn fold_file_name(name: &OsStr) -> Vec<u16> {
    name.encode_wide()
        .map(|unit| unsafe { windows_sys::Wdk::System::SystemServices::RtlUpcaseUnicodeChar(unit) })
        .collect()
}
