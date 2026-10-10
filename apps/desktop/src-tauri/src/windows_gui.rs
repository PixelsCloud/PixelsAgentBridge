//! Keep one Desktop window per Windows user session and restore it on relaunch.
use std::{
    fs::{File, OpenOptions, TryLockError},
    io,
    path::Path,
    sync::Arc,
    time::Duration,
};

use sha2::{Digest, Sha256};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
    System::{
        RemoteDesktop::ProcessIdToSessionId,
        Threading::{
            CreateEventW, EVENT_MODIFY_STATE, GetCurrentProcessId, INFINITE, OpenEventW,
            SetEvent, WaitForSingleObject,
        },
    },
};

struct EventHandle(isize);

impl EventHandle {
    fn raw(&self) -> HANDLE { self.0 as HANDLE }
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.raw()); }
    }
}

pub struct GuiInstance {
    // The lock remains held for the complete GUI lifetime.
    _lock: File,
    event: Arc<EventHandle>,
}

fn event_name(root: &Path, session: u32) -> Vec<u16> {
    let digest = Sha256::digest(root.to_string_lossy().as_bytes());
    let suffix: String = digest[..8].iter().map(|byte| format!("{byte:02x}")).collect();
    format!("Local\\PixelsAgentBridgeDesktopShow-{session}-{suffix}\0")
        .encode_utf16().collect()
}

fn session_id() -> io::Result<u32> {
    let mut session = 0;
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(session)
}

impl GuiInstance {
    pub fn acquire(root: &Path) -> io::Result<Option<Self>> {
        std::fs::create_dir_all(root)?;
        let session = session_id()?;
        let lock = OpenOptions::new().read(true).write(true).create(true)
            .open(root.join(format!("desktop-{session}.lock")))?;
        let name = event_name(root, session);
        match lock.try_lock() {
            Ok(()) => {
                let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
                if event.is_null() { return Err(io::Error::last_os_error()); }
                Ok(Some(Self { _lock: lock, event: Arc::new(EventHandle(event as isize)) }))
            }
            Err(TryLockError::WouldBlock) => {
                // The owner can hold the lock for a moment before it creates the event.
                for _ in 0..40 {
                    let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
                    if !event.is_null() {
                        let sent = unsafe { SetEvent(event) };
                        unsafe { CloseHandle(event); }
                        if sent == 0 { return Err(io::Error::last_os_error()); }
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(io::Error::new(io::ErrorKind::TimedOut, "existing Desktop did not accept show request"))
            }
            Err(TryLockError::Error(error)) => Err(error),
        }
    }

    pub fn listen(&self, handle: tauri::AppHandle) -> io::Result<()> {
        let event = self.event.clone();
        std::thread::Builder::new().name("desktop-launch".into()).spawn(move || {
            while unsafe { WaitForSingleObject(event.raw(), INFINITE) } == WAIT_OBJECT_0 {
                let app = handle.clone();
                let _ = handle.run_on_main_thread(move || crate::tray::show_window(&app));
            }
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_launch_signals_owner_and_recovers_after_exit() {
        let root = tempfile::tempdir().unwrap();
        let owner = GuiInstance::acquire(root.path()).unwrap().unwrap();
        for _ in 0..3 {
            assert!(GuiInstance::acquire(root.path()).unwrap().is_none());
            assert_eq!(unsafe { WaitForSingleObject(owner.event.raw(), 1000) }, WAIT_OBJECT_0);
        }
        drop(owner);
        assert!(GuiInstance::acquire(root.path()).unwrap().is_some());
    }
}
