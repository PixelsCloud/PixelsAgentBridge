use std::io;

#[cfg(windows)]
mod windows {
    use std::{
        ffi::OsStr,
        io,
        os::windows::ffi::OsStrExt,
        sync::atomic::{AtomicBool, Ordering},
    };

    use windows_sys::Win32::{
        Foundation::FreeLibrary,
        System::LibraryLoader::{GetProcAddress, LoadLibraryW},
    };

    static SERVICE_CONTEXT: AtomicBool = AtomicBool::new(false);

    pub fn set_service_context(enabled: bool) {
        SERVICE_CONTEXT.store(enabled, Ordering::Release);
    }

    pub fn send() -> io::Result<()> {
        if !SERVICE_CONTEXT.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "secure attention requires the Windows Executor service",
            ));
        }
        let dll = OsStr::new("sas.dll")
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let library = unsafe { LoadLibraryW(dll.as_ptr()) };
        if library.is_null() {
            return Err(io::Error::last_os_error());
        }
        let address = unsafe { GetProcAddress(library, b"SendSAS\0".as_ptr()) };
        let result = if let Some(address) = address {
            let send_sas: unsafe extern "system" fn(i32) = unsafe { std::mem::transmute(address) };
            // The caller is the LocalSystem Windows service, not an interactive user.
            unsafe { send_sas(0) };
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        };
        unsafe { FreeLibrary(library) };
        result
    }
}

#[cfg(windows)]
pub use windows::set_service_context;

pub fn send_secure_attention() -> io::Result<()> {
    #[cfg(windows)]
    {
        windows::send()
    }
    #[cfg(not(windows))]
    {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Windows only"))
    }
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn rejects_calls_outside_the_service_context() {
        let error = super::send_secure_attention().unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }
}
