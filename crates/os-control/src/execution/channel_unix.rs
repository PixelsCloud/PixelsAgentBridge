use super::{UserIdentity, WorkerStream};
use std::{
    ffi::CString,
    fs::{self, DirBuilder},
    io,
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, PermissionsExt},
    },
    path::PathBuf,
};
use tokio::net::{UnixListener, UnixStream};

pub(super) struct Listener {
    listener: UnixListener,
    directory: PathBuf,
    address: String,
    uid: u32,
}
impl Listener {
    pub fn bind(target: &UserIdentity) -> io::Result<Self> {
        let uid: u32 = target
            .account_id
            .strip_prefix("uid:")
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid worker UID"))?;
        // /tmp is reachable by another account; the service's TMPDIR need not be.
        // Exclusive mkdir under the system-owned sticky parent prevents reuse.
        let directory =
            PathBuf::from("/tmp").join(format!("pab-worker-{}", pab_protocol::RequestId::new()));
        DirBuilder::new().mode(0o700).create(&directory)?;
        let address = directory.join("channel").to_string_lossy().into_owned();
        let result = (|| {
            let listener = UnixListener::bind(&address)?;
            let path = CString::new(address.as_bytes())?;
            // SAFETY: path is a live NUL-terminated socket pathname inside our
            // still-private new directory; gid -1 preserves the existing group.
            if unsafe { libc::chown(path.as_ptr(), uid, !0) } != 0 {
                return Err(io::Error::last_os_error());
            }
            fs::set_permissions(&address, fs::Permissions::from_mode(0o600))?;
            // The target can reach the socket but cannot replace its directory
            // entry. No data files or command payloads are stored here.
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o711))?;
            Ok(Self {
                listener,
                directory: directory.clone(),
                address: address.clone(),
                uid,
            })
        })();
        if result.is_err() {
            let _ = fs::remove_file(&address);
            let _ = fs::remove_dir(&directory);
        }
        result
    }
    pub fn address(&self) -> &str {
        &self.address
    }
    pub async fn accept(&mut self, child_pid: u32) -> io::Result<WorkerStream> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            let peer = match stream.peer_cred() {
                Ok(peer) => peer,
                // Darwin may discard peer credentials once a rejected client
                // has already closed. Its disappearance must not fail the real
                // worker's still-pending handshake.
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotConnected | io::ErrorKind::ConnectionReset
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            };
            if peer.uid() == self.uid
                && peer.pid().and_then(|p| u32::try_from(p).ok()) == Some(child_pid)
            {
                return Ok(Box::new(stream));
            }
        }
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        // Delete only our socket and empty directory, never recursively.
        let _ = fs::remove_file(&self.address);
        let _ = fs::remove_dir(&self.directory);
    }
}
pub(super) async fn connect(address: &str, parent_pid: u32) -> io::Result<WorkerStream> {
    let path = PathBuf::from(address);
    if !path.is_absolute() || path.as_os_str().as_bytes().contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid worker socket path",
        ));
    }
    let stream = UnixStream::connect(path).await?;
    if stream
        .peer_cred()?
        .pid()
        .and_then(|p| u32::try_from(p).ok())
        != Some(parent_pid)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "worker IPC server PID mismatch",
        ));
    }
    Ok(Box::new(stream))
}
