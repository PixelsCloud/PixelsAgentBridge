use super::{UserIdentity, WorkerStream};
use std::{io, os::windows::io::AsRawHandle, ptr, time::Duration};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
use windows_sys::Win32::{
    Foundation::{ERROR_PIPE_BUSY, LocalFree},
    Security::{
        Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW,
            SDDL_REVISION_1,
        },
        SECURITY_ATTRIBUTES,
    },
    System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId},
};

pub(super) struct Listener {
    pipe: Option<NamedPipeServer>,
    address: String,
    sddl: Vec<u16>,
}
impl Listener {
    pub fn bind(target: &UserIdentity) -> io::Result<Self> {
        let parent = super::super::current_identity()?;
        // Validate both SIDs natively before interpolating them into SDDL.
        for sid in [&target.account_id, &parent.account_id] {
            if !sid.starts_with("S-1-")
                || !sid
                    .bytes()
                    .all(|c| c.is_ascii_digit() || c == b'S' || c == b'-')
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid worker SID",
                ));
            }
            let text = wide(sid);
            let mut raw = ptr::null_mut();
            if unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut raw) } == 0 {
                return Err(io::Error::last_os_error());
            }
            unsafe {
                LocalFree(raw);
            }
        }
        let sddl = wide(&format!(
            "D:P(A;;GA;;;{})(A;;GRGW;;;{})",
            parent.account_id, target.account_id
        ));
        let address = format!(r"\\.\pipe\pab-worker-{}", pab_protocol::RequestId::new());
        let pipe = create_pipe(&address, &sddl, true)?;
        Ok(Self {
            pipe: Some(pipe),
            address,
            sddl,
        })
    }
    pub fn address(&self) -> &str {
        &self.address
    }
    pub async fn accept(&mut self, child_pid: u32) -> io::Result<WorkerStream> {
        loop {
            let pipe = self
                .pipe
                .as_mut()
                .ok_or_else(|| io::Error::other("worker channel already accepted"))?;
            pipe.connect().await?;
            let mut pid = 0;
            if unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle().cast(), &mut pid) } == 0 {
                let error = io::Error::last_os_error();
                let _ = pipe.disconnect();
                return Err(error);
            }
            if pid == child_pid {
                return Ok(Box::new(self.pipe.take().unwrap()));
            }
            // A disconnected mio pipe can retain EOF/read readiness from the
            // rejected client. Create a fresh instance while the old one still
            // owns the name, then drop the old instance (no namespace gap).
            let next = create_pipe(&self.address, &self.sddl, false)?;
            self.pipe = Some(next);
        }
    }
}
pub(super) async fn connect(address: &str, parent_pid: u32) -> io::Result<WorkerStream> {
    if !address.starts_with(r"\\.\pipe\pab-worker-") || address.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid worker pipe address",
        ));
    }
    let stream = loop {
        // Tokio defaults to SECURITY_IDENTIFICATION, not impersonation.
        match ClientOptions::new().open(address) {
            Ok(value) => break value,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                tokio::time::sleep(Duration::from_millis(20)).await
            }
            Err(e) => return Err(e),
        }
    };
    let mut pid = 0;
    if unsafe { GetNamedPipeServerProcessId(stream.as_raw_handle().cast(), &mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if pid != parent_pid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "worker IPC server PID mismatch",
        ));
    }
    Ok(Box::new(stream))
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn create_pipe(address: &str, sddl: &[u16], first: bool) -> io::Result<NamedPipeServer> {
    let mut descriptor = ptr::null_mut();
    // SAFETY: validated SIDs, live UTF-16 text and writable outputs. Windows
    // copies this descriptor during pipe creation; LocalFree follows either result.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let result = unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .max_instances(2)
            .create_with_security_attributes_raw(
                address,
                (&mut attrs as *mut SECURITY_ATTRIBUTES).cast(),
            )
    };
    unsafe {
        LocalFree(descriptor);
    }
    result
}
