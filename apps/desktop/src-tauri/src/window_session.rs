use pab_protocol::WindowEntry;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub fn list_windows() -> Vec<WindowEntry> {
    let Ok(windows) = xcap::Window::all() else {
        return vec![];
    };
    let mut entries: Vec<_> = windows
        .into_iter()
        .filter_map(|window| {
            let title = window.title().ok()?;
            if title.trim().is_empty() {
                return None;
            }
            let mut end = title.len().min(pab_protocol::MAX_WINDOW_TITLE_BYTES);
            while !title.is_char_boundary(end) {
                end -= 1;
            }
            Some(WindowEntry {
                title: title[..end].into(),
                process_id: window.pid().ok()?,
            })
        })
        .collect();
    entries.sort_by(|a, b| a.title.cmp(&b.title));
    entries.truncate(pab_protocol::MAX_WINDOW_ENTRIES);
    entries
}
#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub fn list_windows() -> Vec<WindowEntry> {
    vec![]
}
