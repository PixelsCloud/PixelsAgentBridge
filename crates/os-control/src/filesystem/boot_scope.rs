//! Native boot/installation namespace for comparing filesystem observations
//! across processes. This value is neither an authorization token nor a device
//! identifier, and consumers transmit only its domain-separated digest.
use std::io;

pub fn filesystem_boot_scope() -> io::Result<[u8; 16]> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::RtlNtStatusToDosError,
            System::LibraryLoader::{GetModuleHandleW, GetProcAddress},
        };
        #[repr(C)]
        struct BootEnvironment {
            identifier: [u8; 16],
            firmware: u32,
            reserved: u32,
            flags: u64,
        }
        type Query = unsafe extern "system" fn(i32, *mut core::ffi::c_void, u32, *mut u32) -> i32;
        let module_name: Vec<u16> = "ntdll.dll".encode_utf16().chain(Some(0)).collect();
        // ntdll is already loaded for the lifetime of a Windows process. Resolve
        // the optional native query dynamically and fail closed if unavailable.
        let module = unsafe { GetModuleHandleW(module_name.as_ptr()) };
        if module.is_null() {
            return Err(io::Error::last_os_error());
        }
        let address =
            unsafe { GetProcAddress(module, c"NtQuerySystemInformation".as_ptr().cast()) }
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::Unsupported,
                        "native filesystem scope unavailable",
                    )
                })?;
        let query: Query = unsafe { std::mem::transmute(address) };
        let mut value = BootEnvironment {
            identifier: [0; 16],
            firmware: 0,
            reserved: 0,
            flags: 0,
        };
        let mut length = 0;
        // SystemBootEnvironmentInformation (90), the Windows 8+ 32-byte layout.
        // Both buffers remain live, correctly aligned, and bounded for the call.
        let status = unsafe {
            query(
                90,
                (&mut value as *mut BootEnvironment).cast(),
                std::mem::size_of::<BootEnvironment>() as u32,
                &mut length,
            )
        };
        if status < 0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { RtlNtStatusToDosError(status) } as i32,
            ));
        }
        if length < 24 || value.identifier == [0; 16] {
            return Err(io::Error::other("invalid native filesystem scope"));
        }
        Ok(value.identifier)
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::io::Read;
        let mut value = Vec::new();
        std::fs::File::open("/proc/sys/kernel/random/boot_id")?
            .take(128)
            .read_to_end(&mut value)?;
        parse_uuid(&value)
    }
    #[cfg(target_os = "macos")]
    {
        let mut value = [0u8; 128];
        let mut length = value.len();
        let result = unsafe {
            libc::sysctlbyname(
                c"kern.bootsessionuuid".as_ptr(),
                value.as_mut_ptr().cast(),
                &mut length,
                std::ptr::null_mut(),
                0,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        if length > value.len() {
            return Err(io::Error::other("invalid native filesystem scope length"));
        }
        parse_uuid(&value[..length])
    }
}
#[cfg(unix)]
fn parse_uuid(bytes: &[u8]) -> io::Result<[u8; 16]> {
    let text = std::str::from_utf8(bytes)
        .map_err(io::Error::other)?
        .trim_matches(|c: char| c.is_ascii_whitespace() || c == '\0');
    if text.len() != 36
        || [8, 13, 18, 23]
            .iter()
            .any(|index| text.as_bytes()[*index] != b'-')
    {
        return Err(io::Error::other("invalid native filesystem scope"));
    }
    let hex: String = text.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::other("invalid native filesystem scope"));
    }
    let mut result = [0; 16];
    for (index, value) in result.iter_mut().enumerate() {
        *value =
            u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).map_err(io::Error::other)?;
    }
    if result == [0; 16] {
        return Err(io::Error::other("empty native filesystem scope"));
    }
    Ok(result)
}
