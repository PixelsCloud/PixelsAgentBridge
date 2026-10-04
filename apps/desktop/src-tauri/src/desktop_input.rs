#[cfg(not(target_os = "linux"))]
use pab_protocol::DesktopInputEvent;

#[cfg(windows)]
pub fn apply(event: DesktopInputEvent) -> Result<(), String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
        MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
        MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, SetCursorPos,
    };

    let input = match event {
        DesktopInputEvent::MouseMove { x, y } => {
            let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
            let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
            if width <= 0 || height <= 0 {
                return Err("primary display is unavailable".to_owned());
            }
            let pixel_x = i64::from(x) * i64::from(width - 1) / 65535;
            let pixel_y = i64::from(y) * i64::from(height - 1) / 65535;
            // SAFETY: The coordinates are bounded by the primary display dimensions.
            if unsafe { SetCursorPos(pixel_x as i32, pixel_y as i32) } == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            return Ok(());
        }
        DesktopInputEvent::MouseButton { button, down } => {
            let flags = match (button, down) {
                (pab_protocol::DesktopMouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
                (pab_protocol::DesktopMouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
                (pab_protocol::DesktopMouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
                (pab_protocol::DesktopMouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
                (pab_protocol::DesktopMouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
                (pab_protocol::DesktopMouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
            };
            INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            }
        }
        DesktopInputEvent::MouseWheel { delta } => INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    mouseData: i32::from(delta) as u32,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    ..Default::default()
                },
            },
        },
        DesktopInputEvent::Key { virtual_key, down } => {
            if virtual_key == 0 || virtual_key > 254 {
                return Err("invalid virtual key".to_owned());
            }
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: virtual_key,
                        dwFlags: if down { 0 } else { KEYEVENTF_KEYUP },
                        ..Default::default()
                    },
                },
            }
        }
        DesktopInputEvent::SecureAttention => {
            return Err("secure attention must be handled by the Executor service".to_owned());
        }
    };
    // SAFETY: input is a fully initialized INPUT and SendInput reads only this slice.
    if unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) } != 1 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::apply;

#[cfg(target_os = "macos")]
pub fn apply(event: DesktopInputEvent) -> Result<(), String> {
    pab_desktop_control::apply_input(event)
}

#[cfg(target_os = "macos")]
pub struct InputGuard;
#[cfg(target_os = "macos")]
impl Drop for InputGuard {
    fn drop(&mut self) {
        pab_desktop_control::release_input();
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn apply(_event: DesktopInputEvent) -> Result<(), String> {
    Err("desktop input is not supported on this platform".to_owned())
}
