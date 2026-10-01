use pab_protocol::WindowControlAction;
use windows_sys::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};
pub const BACKEND: &str = "xcap/windows-sys/enigo";
// Windows USER handles are 32-bit signed values even in 64-bit callers.
fn hwnd(id: u32) -> HWND {
    id as i32 as isize as HWND
}
fn key(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(Some(0)).collect()
}
fn error() -> String {
    std::io::Error::last_os_error().to_string()
}
pub fn mark(id: u32, pid: u32, name: &str, marker: u32) -> Result<(), String> {
    let mut actual = 0;
    // SAFETY: native functions validate the external HWND; strings are NUL-terminated.
    unsafe {
        if IsWindow(hwnd(id)) == 0
            || GetWindowThreadProcessId(hwnd(id), &mut actual) == 0
            || actual != pid
        {
            return Err("window identity changed during enumeration".into());
        }
        if SetPropW(hwnd(id), key(name).as_ptr(), marker as usize as _) == 0 {
            return Err(format!("cannot establish window identity: {}", error()));
        }
    }
    verify(id, pid, name, marker)
}
pub fn verify(id: u32, pid: u32, name: &str, marker: u32) -> Result<(), String> {
    let mut actual = 0;
    // SAFETY: no external pointers are dereferenced. Property data is an integer marker.
    unsafe {
        if IsWindow(hwnd(id)) == 0
            || GetWindowThreadProcessId(hwnd(id), &mut actual) == 0
            || actual != pid
            || GetPropW(hwnd(id), key(name).as_ptr()) as usize != marker as usize
        {
            return Err(
                "window reference invalidated (closed, replaced or permissions changed)".into(),
            );
        }
    }
    Ok(())
}
pub fn unmark(id: u32, pid: u32, name: &str, marker: u32) {
    if verify(id, pid, name, marker).is_ok() {
        unsafe {
            RemovePropW(hwnd(id), key(name).as_ptr());
        }
    }
}
pub fn focused(id: u32) -> Result<bool, String> {
    Ok(unsafe { GetForegroundWindow() == hwnd(id) })
}
pub fn act(
    id: u32,
    pid: u32,
    name: &str,
    marker: u32,
    action: Option<WindowControlAction>,
) -> Result<(), String> {
    verify(id, pid, name, marker)?;
    let target = hwnd(id);
    // SAFETY: target has just passed identity checks; APIs validate its current handle.
    unsafe {
        match action {
            None => {
                if GetForegroundWindow() == target {
                    return Ok(());
                }
                if SetForegroundWindow(target) == 0 {
                    return Err(
                        "Windows foreground policy refused focus; no bypass attempted".into(),
                    );
                }
            }
            Some(WindowControlAction::Minimize) => {
                if ShowWindowAsync(target, SW_MINIMIZE) == 0 {
                    return Err(error());
                }
            }
            Some(WindowControlAction::Maximize) => {
                if ShowWindowAsync(target, SW_MAXIMIZE) == 0 {
                    return Err(error());
                }
            }
            Some(WindowControlAction::Restore) => {
                if ShowWindowAsync(target, SW_RESTORE) == 0 {
                    return Err(error());
                }
            }
            Some(WindowControlAction::Close) => {
                if PostMessageW(target, WM_CLOSE, 0, 0) == 0 {
                    return Err(error());
                }
            }
        }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let exists = unsafe { IsWindow(target) != 0 };
        if action == Some(WindowControlAction::Close) && !exists {
            return Ok(());
        }
        verify(id, pid, name, marker)?;
        let confirmed = unsafe {
            match action {
                None => GetForegroundWindow() == target,
                Some(WindowControlAction::Minimize) => IsIconic(target) != 0,
                Some(WindowControlAction::Maximize) => IsZoomed(target) != 0,
                Some(WindowControlAction::Restore) => {
                    IsIconic(target) == 0 && IsZoomed(target) == 0
                }
                Some(WindowControlAction::Close) => false,
            }
        };
        if confirmed {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err("window action submitted but requested state not observed; application may show a dialog or refuse".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
