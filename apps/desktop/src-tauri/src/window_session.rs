use pab_protocol::WindowEntry;

#[cfg(windows)]
pub fn list_windows() -> Vec<WindowEntry> {
    use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;

    let mut entries = Vec::<WindowEntry>::new();
    // EnumWindows calls the callback synchronously; the pointer remains valid for this call.
    unsafe {
        let _ = EnumWindows(
            Some(collect_window),
            (&mut entries as *mut Vec<WindowEntry>) as isize,
        );
    }
    entries.sort_by(|left, right| left.title.cmp(&right.title));
    entries.truncate(pab_protocol::MAX_WINDOW_ENTRIES);
    entries
}

#[cfg(windows)]
unsafe extern "system" fn collect_window(
    hwnd: windows_sys::Win32::Foundation::HWND,
    context: isize,
) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };

    if unsafe { IsWindowVisible(hwnd) } == 0 {
        return 1;
    }
    let title_length = unsafe { GetWindowTextLengthW(hwnd) };
    if title_length <= 0 {
        return 1;
    }
    let mut title = vec![0u16; (title_length as usize).min(1024) + 1];
    let copied = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
    if copied <= 0 {
        return 1;
    }
    let title = String::from_utf16_lossy(&title[..copied as usize]);
    if title.trim().is_empty() {
        return 1;
    }
    let title = title.chars().take(256).collect::<String>();
    let title = if title.len() > pab_protocol::MAX_WINDOW_TITLE_BYTES {
        title.chars().take(128).collect()
    } else {
        title
    };
    let mut process_id = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) };
    // SAFETY: context is the mutable Vec pointer passed to synchronous EnumWindows above.
    let entries = unsafe { &mut *(context as *mut Vec<WindowEntry>) };
    entries.push(WindowEntry { title, process_id });
    1
}

#[cfg(not(windows))]
pub fn list_windows() -> Vec<WindowEntry> {
    Vec::new()
}
