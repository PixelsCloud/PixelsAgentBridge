use std::io;

#[cfg(windows)]
pub use pab_windows_sas::set_service_context;

pub(crate) fn send_secure_attention() -> io::Result<()> {
    #[cfg(windows)]
    {
        pab_windows_sas::send_secure_attention()
    }
    #[cfg(not(windows))]
    {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Windows only"))
    }
}
