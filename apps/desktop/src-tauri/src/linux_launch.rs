//! One graphical instance per user; repeat launches restore its window.
use std::{
    fs::{File, OpenOptions, TryLockError},
    io,
    os::unix::{fs::OpenOptionsExt, net::UnixDatagram},
    path::Path,
    time::Duration,
};

pub struct GuiInstance {
    _lock: File,
    socket: UnixDatagram,
}

impl GuiInstance {
    pub fn acquire(root: &Path) -> io::Result<Option<Self>> {
        pab_agent_core::ensure_data_dir(root)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(root.join("desktop.lock"))?;
        let address = root.join("desktop.sock");
        match lock.try_lock() {
            Ok(()) => {
                // Only the lock owner can remove an endpoint left by a crashed GUI.
                match std::fs::remove_file(&address) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
                Ok(Some(Self {
                    _lock: lock,
                    socket: UnixDatagram::bind(address)?,
                }))
            }
            Err(TryLockError::WouldBlock) => {
                let socket = UnixDatagram::unbound()?;
                socket.set_write_timeout(Some(Duration::from_secs(1)))?;
                for attempt in 0..40 {
                    match socket.send_to(b"show", &address) {
                        Ok(_) => return Ok(None),
                        Err(e)
                            if attempt < 39
                                && matches!(
                                    e.kind(),
                                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                                ) =>
                        {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Err(e) => return Err(e),
                    }
                }
                unreachable!()
            }
            Err(TryLockError::Error(e)) => Err(e),
        }
    }

    pub fn listen(&self, app: tauri::AppHandle) -> io::Result<()> {
        let socket = self.socket.try_clone()?;
        std::thread::Builder::new()
            .name("desktop-launch".into())
            .spawn(move || {
                let mut message = [0; 16];
                while let Ok(length) = socket.recv(&mut message) {
                    if &message[..length] == b"show" {
                        let handle = app.clone();
                        let _ = app.run_on_main_thread(move || crate::tray::show_window(&handle));
                    }
                }
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeat_launch_signals_owner_and_crash_endpoint_is_recovered() {
        let directory = tempfile::tempdir().unwrap();
        let first = GuiInstance::acquire(directory.path()).unwrap().unwrap();
        assert!(GuiInstance::acquire(directory.path()).unwrap().is_none());
        first
            .socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut buffer = [0; 16];
        let length = first.socket.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..length], b"show");
        drop(first);
        assert!(directory.path().join("desktop.sock").exists());
        assert!(GuiInstance::acquire(directory.path()).unwrap().is_some());
    }
}
