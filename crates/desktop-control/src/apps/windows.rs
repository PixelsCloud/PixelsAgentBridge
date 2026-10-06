use super::*;
#[path = "windows_action.rs"]
mod action;
pub(super) use action::act;
use pab_os_control::execution::UserIdentity;
use windows::{
    Win32::{
        Foundation::*,
        System::{Com::*, Threading::*},
        UI::Shell::*,
    },
    core::PWSTR,
};

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}
fn name(item: &IShellItem, kind: SIGDN) -> Result<String, String> {
    // Shell allocates the returned string with the COM task allocator.
    unsafe {
        let ptr = item.GetDisplayName(kind).map_err(|e| e.to_string())?;
        let value = ptr.to_string().map_err(|e| e.to_string());
        CoTaskMemFree(Some(ptr.0.cast()));
        let value = value?;
        if value.len() > 4096 {
            return Err("application metadata exceeds limit".into());
        }
        Ok(value)
    }
}
pub(super) fn list(
    request: &AppListRequest,
    identity: &UserIdentity,
    snapshot: &mut AppListSnapshot,
) -> Result<(), String> {
    if request.scope == AppListScope::Running {
        return running(identity, snapshot);
    }
    // Catalog enumeration is isolated on its caller's worker thread.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
    }
    let _apartment = Apartment;
    let folder: IShellItem =
        unsafe { SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None) }
            .map_err(|e| e.to_string())?;
    let items: IEnumShellItems =
        unsafe { folder.BindToHandler(None, &BHID_EnumItems) }.map_err(|e| e.to_string())?;
    snapshot.sources.push("windows_apps_folder".into());
    let mut skipped = 0;
    for index in 0..4097 {
        let mut batch = [None];
        let mut fetched = 0;
        unsafe { items.Next(&mut batch, Some(&mut fetched)) }.map_err(|e| e.to_string())?;
        if fetched == 0 {
            break;
        }
        if index == 4096 {
            snapshot.truncated = true;
            break;
        }
        let Some(item) = batch[0].take() else {
            continue;
        };
        match (
            name(&item, SIGDN_NORMALDISPLAY),
            name(&item, SIGDN_DESKTOPABSOLUTEPARSING),
        ) {
            (Ok(name), Ok(id)) => snapshot.apps.push(AppInfo {
                name,
                app_id: Some(id),
                path: name_path(&item),
                source: "windows_apps_folder".into(),
                instance: None,
            }),
            _ => skipped += 1,
        }
    }
    if skipped > 0 {
        snapshot.warnings.push(format!(
            "{skipped} catalog entries had unavailable or oversized metadata"
        ));
    }
    Ok(())
}
fn name_path(item: &IShellItem) -> Option<String> {
    name(item, SIGDN_FILESYSPATH).ok()
}

fn running(identity: &UserIdentity, snapshot: &mut AppListSnapshot) -> Result<(), String> {
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM},
        UI::WindowsAndMessaging::*,
    };
    struct WindowPids {
        values: std::collections::BTreeSet<u32>,
        truncated: bool,
    }
    unsafe extern "system" fn collect(window: HWND, data: LPARAM) -> i32 {
        // EnumWindows invokes synchronously; the caller-owned collection stays live.
        let target = unsafe { &mut *(data as *mut WindowPids) };
        if unsafe { IsWindowVisible(window) } != 0 {
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(window, &mut pid);
            }
            if target.values.len() >= 4096 {
                target.truncated = true;
                return 0;
            }
            if pid != 0 {
                target.values.insert(pid);
            }
        }
        1
    }
    let mut pids = WindowPids {
        values: Default::default(),
        truncated: false,
    };
    let ok = unsafe { EnumWindows(Some(collect), (&mut pids as *mut WindowPids) as LPARAM) };
    if ok == 0 && !pids.truncated {
        return Err(std::io::Error::last_os_error().to_string());
    }
    snapshot.truncated |= pids.truncated;
    snapshot
        .sources
        .push("visible_windows_in_helper_session".into());
    let mut skipped = 0;
    for pid in pids.values {
        let mut session = 0;
        if unsafe {
            windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(pid, &mut session)
        } == 0
            || Some(session) != identity.session_id
        {
            continue;
        }
        let Ok(marker) = pab_os_control::process_identity(pid) else {
            skipped += 1;
            continue;
        };
        let path = unsafe {
            let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                skipped += 1;
                continue;
            };
            let mut buffer = vec![0u16; 32768];
            let mut size = buffer.len() as u32;
            let result = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut size,
            );
            let _ = CloseHandle(handle);
            if result.is_err() {
                skipped += 1;
                continue;
            }
            String::from_utf16_lossy(&buffer[..size as usize])
        };
        if path.len() > 4096 || pab_os_control::process_identity(pid).ok().as_ref() != Some(&marker)
        {
            skipped += 1;
            continue;
        }
        let name = std::path::Path::new(&path)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        snapshot.apps.push(AppInfo {
            name,
            app_id: None,
            path: Some(path),
            source: "visible_windows_in_helper_session".into(),
            instance: Some(AppInstance {
                process_id: pid,
                process_identity: marker,
                account_id: None,
                session_id: Some(session.to_string()),
            }),
        });
    }
    if skipped > 0 {
        snapshot.warnings.push(format!(
            "{skipped} process instances disappeared or could not be inspected"
        ));
    }
    Ok(())
}
