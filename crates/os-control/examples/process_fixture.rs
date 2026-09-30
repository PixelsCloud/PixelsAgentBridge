//! Hidden window test process. Not included in installers.
#[cfg(not(windows))]
fn main() {
    std::thread::sleep(std::time::Duration::from_secs(60));
}
#[cfg(windows)]
fn main() {
    use windows_sys::Win32::{System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::*};
    let mode = std::env::args().nth(1).unwrap();
    let ready = std::env::args().nth(2).unwrap();
    let class: Vec<u16> = "PabC3HiddenFixture\0".encode_utf16().collect();
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let config = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..Default::default()
        };
        assert_ne!(RegisterClassW(&config), 0);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        assert!(!hwnd.is_null());
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            if mode == "ignore-close" { 1 } else { 0 },
        );
        std::fs::write(ready, "ready").unwrap();
        let mut message = MSG::default();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
#[cfg(windows)]
unsafe extern "system" fn window_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    w: usize,
    l: isize,
) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    if message == WM_CLOSE && unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } == 1 {
        return 0;
    }
    if message == WM_DESTROY {
        unsafe {
            PostQuitMessage(0);
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, message, w, l) }
}
