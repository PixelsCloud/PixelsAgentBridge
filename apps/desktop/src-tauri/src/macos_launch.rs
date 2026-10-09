//! Keep the launchd helper hidden and route every GUI launch to one window.
use std::{
    fs::{File, OpenOptions, TryLockError},
    io,
    os::unix::{fs::OpenOptionsExt, net::UnixDatagram},
    path::Path,
    time::Duration,
};

use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol};

pub struct GuiInstance {
    // Keep the lock for the entire GUI lifetime, including startup and shutdown.
    _lock: File,
    socket: UnixDatagram,
}

impl GuiInstance {
    pub fn acquire(root: &Path) -> io::Result<Option<Self>> {
        std::fs::create_dir_all(root)?;
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
                // Only the lock owner may remove a socket left by a crashed GUI.
                match std::fs::remove_file(&address) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                Ok(Some(Self {
                    _lock: lock,
                    socket: UnixDatagram::bind(address)?,
                }))
            }
            Err(TryLockError::WouldBlock) => {
                let socket = UnixDatagram::unbound()?;
                socket.set_write_timeout(Some(Duration::from_secs(1)))?;
                // Another launch may hold the lock before its socket is bound.
                for attempt in 0..40 {
                    match socket.send_to(b"show", &address) {
                        Ok(_) => return Ok(None),
                        Err(error)
                            if attempt < 39
                                && matches!(
                                    error.kind(),
                                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                                ) =>
                        {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Err(error) => return Err(error),
                    }
                }
                unreachable!()
            }
            Err(TryLockError::Error(error)) => Err(error),
        }
    }

    pub fn listen(&self, handle: tauri::AppHandle) -> io::Result<()> {
        let socket = self.socket.try_clone()?;
        std::thread::Builder::new()
            .name("desktop-launch".into())
            .spawn(move || {
                let mut message = [0; 16];
                while let Ok(length) = socket.recv(&mut message) {
                    if &message[..length] == b"show" {
                        let app = handle.clone();
                        let _ = handle.run_on_main_thread(move || crate::tray::show_window(&app));
                    }
                }
            })?;
        Ok(())
    }
}

fn open_gui(app: &NSApplication) {
    app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    // Launch the same signed executable without helper arguments. GuiInstance
    // forwards to an existing window before AppKit starts, so repeated clicks
    // never create another Dock icon or bind another MCP listener.
    let result = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    });
    match result {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(error) => tracing::error!(%error, "could not open desktop window"),
    }
}

define_class!(
    // NSObject has no subclassing requirements; all callbacks run on AppKit's
    // main thread and the delegate is retained until the helper event loop ends.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    pub struct HelperDelegate;

    unsafe impl NSObjectProtocol for HelperDelegate {}
    unsafe impl NSApplicationDelegate for HelperDelegate {
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn reopen(&self, app: &NSApplication, _visible: bool) -> bool {
            open_gui(app);
            false
        }

        #[unsafe(method(applicationOpenUntitledFile:))]
        fn open_untitled(&self, app: &NSApplication) -> bool {
            open_gui(app);
            true
        }

        #[unsafe(method(applicationDidBecomeActive:))]
        fn became_active(&self, _notification: &NSNotification) {
            NSApplication::sharedApplication(self.mtm())
                .setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
        }
    }
);

impl HelperDelegate {
    pub fn new(main: MainThreadMarker) -> Retained<Self> {
        // SAFETY: NSObject's init takes no arguments and returns this object.
        unsafe { msg_send![Self::alloc(main), init] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_launch_forwards_to_owner_and_recovers_after_exit() {
        let root = tempfile::tempdir().unwrap();
        let owner = GuiInstance::acquire(root.path()).unwrap().unwrap();
        owner
            .socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        for _ in 0..3 {
            assert!(GuiInstance::acquire(root.path()).unwrap().is_none());
            let mut buffer = [0; 16];
            let length = owner.socket.recv(&mut buffer).unwrap();
            assert_eq!(&buffer[..length], b"show");
        }
        drop(owner);
        assert!(GuiInstance::acquire(root.path()).unwrap().is_some());
    }

    #[test]
    fn launch_replaces_stale_socket_without_touching_user_data() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("settings.json"), b"preserved").unwrap();
        let stale = UnixDatagram::bind(root.path().join("desktop.sock")).unwrap();
        drop(stale);
        assert!(GuiInstance::acquire(root.path()).unwrap().is_some());
        assert_eq!(
            std::fs::read(root.path().join("settings.json")).unwrap(),
            b"preserved"
        );
    }
}
