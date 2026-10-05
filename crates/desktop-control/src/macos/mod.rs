//! Public Accessibility APIs with retained window identity; no private WindowServer APIs.
use pab_protocol::WindowControlAction;
use std::{
    collections::HashMap,
    ffi::c_void,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};
mod input;
pub use input::{apply_input, release_idle_input, release_input};
pub const BACKEND: &str = "xcap/macos_accessibility/enigo";
type Ref = *const c_void;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SessionGetInfo(session: u32, actual: *mut u32, attributes: *mut u32) -> i32;
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct Point {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct Size {
    width: f64,
    height: f64,
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: Ref) -> bool;
    static kAXTrustedCheckOptionPrompt: Ref;
    fn getuid() -> u32;
    fn AXUIElementCreateApplication(pid: i32) -> Ref;
    fn AXUIElementCreateSystemWide() -> Ref;
    fn AXUIElementCopyAttributeValue(element: Ref, name: Ref, value: *mut Ref) -> i32;
    fn AXUIElementSetAttributeValue(element: Ref, name: Ref, value: Ref) -> i32;
    fn AXUIElementPerformAction(element: Ref, action: Ref) -> i32;
    fn AXUIElementSetMessagingTimeout(element: Ref, seconds: f32) -> i32;
    fn AXValueGetValue(value: Ref, kind: i32, output: *mut c_void) -> bool;
    fn AXValueCreate(kind: i32, value: *const c_void) -> Ref;
    fn AXValueGetTypeID() -> usize;
    fn AXUIElementGetTypeID() -> usize;
    fn AXUIElementCopyElementAtPosition(element: Ref, x: f32, y: f32, value: *mut Ref) -> i32;
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRetain(value: Ref) -> Ref;
    fn CFRelease(value: Ref);
    fn CFEqual(first: Ref, second: Ref) -> bool;
    fn CFGetTypeID(value: Ref) -> usize;
    fn CFStringCreateWithBytes(
        allocator: Ref,
        bytes: *const u8,
        length: isize,
        encoding: u32,
        external: bool,
    ) -> Ref;
    fn CFStringGetCString(value: Ref, bytes: *mut u8, length: isize, encoding: u32) -> bool;
    fn CFStringGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFArrayGetCount(array: Ref) -> isize;
    fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
    fn CFDictionaryCreate(
        allocator: Ref,
        keys: *const Ref,
        values: *const Ref,
        count: isize,
        key_callbacks: Ref,
        value_callbacks: Ref,
    ) -> Ref;
    static kCFBooleanTrue: Ref;
    static kCFBooleanFalse: Ref;
}

struct Owned(Ref);
// SAFETY: these are retained CF/AX references. Access is serialized by WINDOWS;
// AX remote messaging and CoreFoundation retain/release do not require the main thread.
unsafe impl Send for Owned {}
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}
impl Owned {
    fn new(value: Ref) -> Result<Self, String> {
        if value.is_null() {
            Err("macOS object unavailable".into())
        } else {
            Ok(Self(value))
        }
    }
    fn string(text: &str) -> Self {
        Self::new(unsafe {
            CFStringCreateWithBytes(
                std::ptr::null(),
                text.as_ptr(),
                text.len() as isize,
                0x08000100,
                false,
            )
        })
        .expect("CFString allocation")
    }
    fn attribute(&self, name: &str) -> Result<Self, String> {
        let mut value = std::ptr::null();
        let key = Self::string(name);
        ax(unsafe { AXUIElementCopyAttributeValue(self.0, key.0, &mut value) })?;
        Self::new(value)
    }
    fn set(&self, name: &str, value: Ref) -> Result<(), String> {
        ax(unsafe { AXUIElementSetAttributeValue(self.0, Self::string(name).0, value) })
    }
    fn action(&self, name: &str) -> Result<(), String> {
        ax(unsafe { AXUIElementPerformAction(self.0, Self::string(name).0) })
    }
    fn text(&self) -> Result<String, String> {
        if unsafe { CFGetTypeID(self.0) != CFStringGetTypeID() } {
            return Err("AX attribute is not a string".into());
        }
        let mut bytes = vec![0; 16385];
        if !unsafe {
            CFStringGetCString(self.0, bytes.as_mut_ptr(), bytes.len() as isize, 0x08000100)
        } {
            return Err("AX string exceeds budget".into());
        }
        bytes.truncate(bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len()));
        String::from_utf8(bytes).map_err(|e| e.to_string())
    }
    fn elements(&self) -> Result<Vec<Self>, String> {
        if unsafe { CFGetTypeID(self.0) != CFArrayGetTypeID() } {
            return Err("AX attribute is not an array".into());
        }
        let count = unsafe { CFArrayGetCount(self.0) };
        if !(0..=4096).contains(&count) {
            return Err("AX window scan exceeds budget".into());
        }
        (0..count)
            .map(|i| {
                let value = unsafe { CFArrayGetValueAtIndex(self.0, i) };
                if value.is_null() || unsafe { CFGetTypeID(value) != AXUIElementGetTypeID() } {
                    return Err("invalid AX window element".into());
                }
                Self::new(unsafe { CFRetain(value) })
            })
            .collect()
    }
}
fn ax(code: i32) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!(
            "macOS Accessibility error {code}: permission denied, window unavailable or operation unsupported by application"
        ))
    }
}
pub fn accessibility_allowed() -> bool {
    unsafe { AXIsProcessTrusted() }
}
pub fn active_console() -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(console) = std::fs::metadata("/dev/console") else {
        return false;
    };
    let mut session = 0;
    let mut attributes = 0;
    // callerSecuritySession; sessionHasGraphicAccess | sessionOnConsole.
    let status = unsafe { SessionGetInfo(u32::MAX, &mut session, &mut attributes) };
    interactive_console_matches(unsafe { getuid() }, console.uid(), status, attributes)
}

fn interactive_console_matches(uid: u32, console_uid: u32, status: i32, attributes: u32) -> bool {
    status == 0 && uid == console_uid && attributes & 0x30 == 0x30
}

pub fn login_window_active() -> bool {
    (unsafe { getuid() == 0 }) && active_console()
}
pub fn screen_capture_allowed() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}
/// Called by the foreground application's main thread, never by the daemon.
pub fn request_screen_capture() -> bool {
    screen_capture_allowed() || unsafe { CGRequestScreenCaptureAccess() }
}
/// macOS shows the native prompt asynchronously; false does not mean rejection.
pub fn request_accessibility() -> Result<bool, String> {
    if accessibility_allowed() {
        return Ok(true);
    }
    // Static CF keys/values outlive this dictionary, so no retain callbacks are needed.
    let options = Owned::new(unsafe {
        CFDictionaryCreate(
            std::ptr::null(),
            &kAXTrustedCheckOptionPrompt,
            &kCFBooleanTrue,
            1,
            std::ptr::null(),
            std::ptr::null(),
        )
    })?;
    Ok(unsafe { AXIsProcessTrustedWithOptions(options.0) })
}
pub fn require_screen_capture() -> Result<(), String> {
    if screen_capture_allowed() {
        Ok(())
    } else {
        Err("screen_recording_permission_required: allow Pixels Agent Bridge in System Settings > Privacy & Security > Screen & System Audio Recording, then restart the app/helper".into())
    }
}
pub(super) fn require_accessibility() -> Result<(), String> {
    if accessibility_allowed() {
        Ok(())
    } else {
        Err("accessibility_permission_required: allow Pixels Agent Bridge in System Settings > Privacy & Security > Accessibility".into())
    }
}

fn application(pid: u32) -> Result<Owned, String> {
    require_accessibility()?;
    let app = Owned::new(unsafe {
        AXUIElementCreateApplication(i32::try_from(pid).map_err(|_| "invalid PID")?)
    })?;
    ax(unsafe { AXUIElementSetMessagingTimeout(app.0, 1.0) })?;
    Ok(app)
}
fn geometry(window: &Owned) -> Result<(Point, Size), String> {
    let position = window.attribute("AXPosition")?;
    let size = window.attribute("AXSize")?;
    let mut p = Point::default();
    let mut s = Size::default();
    if unsafe {
        CFGetTypeID(position.0) != AXValueGetTypeID()
            || CFGetTypeID(size.0) != AXValueGetTypeID()
            || !AXValueGetValue(position.0, 1, (&mut p as *mut Point).cast())
            || !AXValueGetValue(size.0, 2, (&mut s as *mut Size).cast())
    } {
        return Err("AX geometry unavailable".into());
    }
    Ok((p, s))
}
fn set_geometry(window: &Owned, p: Point, s: Size) -> Result<(), String> {
    let position = Owned::new(unsafe { AXValueCreate(1, (&p as *const Point).cast()) })?;
    let size = Owned::new(unsafe { AXValueCreate(2, (&s as *const Size).cast()) })?;
    window.set("AXPosition", position.0)?;
    window.set("AXSize", size.0)
}
fn matches_geometry(a: (Point, Size), b: (Point, Size)) -> bool {
    [
        (a.0.x, b.0.x),
        (a.0.y, b.0.y),
        (a.1.width, b.1.width),
        (a.1.height, b.1.height),
    ]
    .into_iter()
    .all(|(x, y)| x.is_finite() && y.is_finite() && (x - y).abs() < 2.0)
}
struct Window {
    id: u32,
    pid: u32,
    process_identity: String,
    element: Owned,
    restore: Option<(Point, Size)>,
}
static WINDOWS: LazyLock<Mutex<HashMap<(String, u32), Window>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn present(w: &Window) -> Result<(), String> {
    if pab_os_control::process_identity(w.pid)? != w.process_identity {
        return Err("window owner process changed".into());
    }
    let app = application(w.pid)?;
    let windows = app.attribute("AXWindows")?.elements()?;
    if windows.iter().any(|e| unsafe { CFEqual(e.0, w.element.0) }) {
        Ok(())
    } else {
        Err("window reference invalidated: closed or replaced".into())
    }
}
pub fn mark(id: u32, pid: u32, key: &str, marker: u32) -> Result<(), String> {
    let process_identity = pab_os_control::process_identity(pid)?;
    let app = application(pid)?;
    let window = xcap::Window::all()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|w| w.id().ok() == Some(id) && w.pid().ok() == Some(pid))
        .ok_or("window disappeared")?;
    let title = window.title().map_err(|e| e.to_string())?;
    let rect = (
        Point {
            x: window.x().map_err(|e| e.to_string())? as f64,
            y: window.y().map_err(|e| e.to_string())? as f64,
        },
        Size {
            width: window.width().map_err(|e| e.to_string())? as f64,
            height: window.height().map_err(|e| e.to_string())? as f64,
        },
    );
    let mut candidates: Vec<_> = app
        .attribute("AXWindows")?
        .elements()?
        .into_iter()
        .filter(|e| {
            e.attribute("AXTitle")
                .and_then(|v| v.text())
                .is_ok_and(|t| t == title)
                && geometry(e).is_ok_and(|g| matches_geometry(g, rect))
        })
        .collect();
    if candidates.len() != 1 {
        return Err(
            "window cannot be uniquely matched to Accessibility; ambiguous/unsupported window"
                .into(),
        );
    }
    ax(unsafe { AXUIElementSetMessagingTimeout(candidates[0].0, 1.0) })?;
    if pab_os_control::process_identity(pid)? != process_identity {
        return Err("window owner changed during registration".into());
    }
    let mut windows = WINDOWS.lock().map_err(|_| "window registry unavailable")?;
    if windows.len() >= 1024 {
        return Err("macOS window registry budget reached".into());
    }
    windows.insert(
        (key.into(), marker),
        Window {
            id,
            pid,
            process_identity,
            element: candidates.remove(0),
            restore: None,
        },
    );
    Ok(())
}
pub fn verify(id: u32, pid: u32, key: &str, marker: u32) -> Result<(), String> {
    let mut windows = WINDOWS.lock().map_err(|_| "window registry unavailable")?;
    let w = windows
        .get(&(key.into(), marker))
        .filter(|w| w.id == id && w.pid == pid)
        .ok_or("stale window reference")?;
    let result = present(w);
    if result.is_err() {
        windows.remove(&(key.into(), marker));
    }
    result
}
pub fn unmark(_: u32, _: u32, key: &str, marker: u32) {
    if let Ok(mut windows) = WINDOWS.lock() {
        windows.remove(&(key.into(), marker));
    }
}
fn is_focused(w: &Window) -> Result<bool, String> {
    present(w)?;
    let system = Owned::new(unsafe { AXUIElementCreateSystemWide() })?;
    let app = system.attribute("AXFocusedApplication")?;
    let focused = app.attribute("AXFocusedWindow")?;
    Ok(unsafe { CFEqual(focused.0, w.element.0) })
}
pub fn focused(id: u32) -> Result<bool, String> {
    let windows = WINDOWS.lock().map_err(|_| "window registry unavailable")?;
    is_focused(
        windows
            .values()
            .find(|w| w.id == id)
            .ok_or("stale window reference")?,
    )
}
pub fn pointer_targets_window(id: u32) -> Result<bool, String> {
    crate::on_input_thread(|| pointer_targets_window_on_main(id))
}

fn pointer_targets_window_on_main(id: u32) -> Result<bool, String> {
    require_accessibility()?;
    use enigo::Mouse;
    let engine = enigo::Enigo::new(&enigo::Settings {
        open_prompt_to_get_permissions: false,
        ..Default::default()
    })
    .map_err(|e| e.to_string())?;
    let (x, y) = engine.location().map_err(|e| e.to_string())?;
    let system = Owned::new(unsafe { AXUIElementCreateSystemWide() })?;
    let mut hit = std::ptr::null();
    ax(unsafe { AXUIElementCopyElementAtPosition(system.0, x as f32, y as f32, &mut hit) })?;
    let hit = Owned::new(hit)?;
    let target = hit.attribute("AXWindow").unwrap_or(hit);
    let windows = WINDOWS.lock().map_err(|_| "window registry unavailable")?;
    let w = windows
        .values()
        .find(|w| w.id == id)
        .ok_or("stale window reference")?;
    present(w)?;
    Ok(unsafe { CFEqual(target.0, w.element.0) })
}
pub fn act(
    id: u32,
    pid: u32,
    key: &str,
    marker: u32,
    action: Option<WindowControlAction>,
) -> Result<(), String> {
    verify(id, pid, key, marker)?;
    let mut windows = WINDOWS.lock().map_err(|_| "window registry unavailable")?;
    let w = windows
        .get_mut(&(key.into(), marker))
        .ok_or("stale window reference")?;
    let mut expected_rect = None;
    match action {
        None => {
            application(pid)?.set("AXFrontmost", unsafe { kCFBooleanTrue })?;
            w.element.action("AXRaise")?;
        }
        Some(WindowControlAction::Minimize) => {
            w.element.set("AXMinimized", unsafe { kCFBooleanTrue })?
        }
        Some(WindowControlAction::Close) => {
            w.element.attribute("AXCloseButton")?.action("AXPress")?
        }
        Some(WindowControlAction::Maximize) => {
            let window = xcap::Window::all()
                .map_err(|e| e.to_string())?
                .into_iter()
                .find(|v| v.id().ok() == Some(id))
                .ok_or("window unavailable")?;
            let monitor = window.current_monitor().map_err(|e| e.to_string())?;
            let rect = (
                Point {
                    x: monitor.x().map_err(|e| e.to_string())? as f64,
                    y: monitor.y().map_err(|e| e.to_string())? as f64,
                },
                Size {
                    width: monitor.width().map_err(|e| e.to_string())? as f64,
                    height: monitor.height().map_err(|e| e.to_string())? as f64,
                },
            );
            if w.restore.is_none() {
                w.restore = Some(geometry(&w.element)?);
            }
            set_geometry(&w.element, rect.0, rect.1)?;
            expected_rect = Some(rect);
        }
        Some(WindowControlAction::Restore) => {
            w.element.set("AXMinimized", unsafe { kCFBooleanFalse })?;
            if let Some(rect) = w.restore {
                set_geometry(&w.element, rect.0, rect.1)?;
                expected_rect = Some(rect);
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        // A failed AX read alone does not prove a close succeeded.
        let app_windows = application(pid)?.attribute("AXWindows")?.elements()?;
        let exists = app_windows
            .iter()
            .any(|e| unsafe { CFEqual(e.0, w.element.0) });
        let observed = match action {
            Some(WindowControlAction::Close) => !exists,
            _ if !exists => return Err("window disappeared during control".into()),
            None => is_focused(w)?,
            Some(WindowControlAction::Minimize) => unsafe {
                CFEqual(w.element.attribute("AXMinimized")?.0, kCFBooleanTrue)
            },
            Some(WindowControlAction::Maximize | WindowControlAction::Restore) => {
                let restored =
                    unsafe { CFEqual(w.element.attribute("AXMinimized")?.0, kCFBooleanFalse) };
                restored
                    && expected_rect
                        .is_none_or(|r| geometry(&w.element).is_ok_and(|g| matches_geometry(g, r)))
            }
        };
        if observed {
            if action == Some(WindowControlAction::Restore) {
                w.restore = None;
            }
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("window action accepted; requested state not observed (application may constrain geometry)".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn console_access_requires_matching_identity_and_active_graphical_session() {
        assert!(interactive_console_matches(501, 501, 0, 0x6030));
        assert!(interactive_console_matches(0, 0, 0, 0x30));
        assert!(!interactive_console_matches(0, 0, 0, 0));
        assert!(!interactive_console_matches(0, 501, 0, 0x30));
        assert!(!interactive_console_matches(501, 0, 0, 0x30));
        assert!(!interactive_console_matches(501, 501, -1, 0x30));
        assert!(!interactive_console_matches(501, 501, 0, 0x10));
        assert!(!interactive_console_matches(501, 501, 0, 0x20));
    }
    #[test]
    fn geometry_rejects_nan_and_large_changes() {
        let g = (
            Point::default(),
            Size {
                width: 100.0,
                height: 100.0,
            },
        );
        assert!(matches_geometry(g, g));
        assert!(!matches_geometry(
            (
                Point {
                    x: f64::NAN,
                    y: 0.0
                },
                g.1
            ),
            g
        ));
    }
    #[test]
    fn permission_preflight_does_not_prompt() {
        let _ = accessibility_allowed();
        let _ = screen_capture_allowed();
    }
}
