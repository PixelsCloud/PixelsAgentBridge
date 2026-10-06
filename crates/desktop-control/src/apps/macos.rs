use super::*;
#[path = "macos_action.rs"]
mod action;
pub(super) use action::act;
use pab_os_control::execution::UserIdentity;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub(super) fn list(
    request: &AppListRequest,
    identity: &UserIdentity,
    snapshot: &mut AppListSnapshot,
) -> Result<(), String> {
    if request.scope == AppListScope::Running {
        return running(identity, snapshot);
    }
    let mut pending = vec![
        (PathBuf::from("/Applications"), 0),
        (PathBuf::from("/System/Applications"), 0),
        (identity.home.join("Applications"), 0),
    ];
    let mut visited = 0;
    let mut skipped = 0;
    snapshot.sources = pending
        .iter()
        .map(|(p, _)| p.to_string_lossy().into_owned())
        .collect();
    while let Some((directory, depth)) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        for entry in entries {
            visited += 1;
            if visited > 4096 {
                snapshot.truncated = true;
                break;
            }
            let Ok(entry) = entry else {
                skipped += 1;
                continue;
            };
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|v| v.eq_ignore_ascii_case("app"))
            {
                match bundle(&path) {
                    Ok(app) => snapshot.apps.push(app),
                    Err(_) => skipped += 1,
                }
            } else if depth < 2 && entry.file_type().is_ok_and(|v| v.is_dir()) {
                pending.push((path, depth + 1));
            }
        }
        if snapshot.truncated {
            break;
        }
    }
    if skipped > 0 {
        snapshot.warnings.push(format!(
            "{skipped} application directories or bundle records could not be read"
        ));
    }
    Ok(())
}

fn bundle(path: &Path) -> Result<AppInfo, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut bytes = Vec::new();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path.join("Contents/Info.plist"))
        .map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > 262144 {
        return Err("bundle metadata must be a bounded regular file".into());
    }
    file.take(262145)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 262144 {
        return Err("bundle metadata exceeds limit".into());
    }
    let plist =
        plist::Value::from_reader(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let info = plist.as_dictionary().ok_or("invalid bundle metadata")?;
    let text = |key| {
        info.get(key)
            .and_then(plist::Value::as_string)
            .filter(|v| !v.is_empty() && v.len() <= 4096)
            .map(str::to_owned)
    };
    let path_text = path
        .to_str()
        .filter(|v| v.len() <= 4096)
        .ok_or("invalid application path")?
        .to_owned();
    Ok(AppInfo {
        name: text("CFBundleDisplayName")
            .or_else(|| text("CFBundleName"))
            .unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            }),
        app_id: text("CFBundleIdentifier"),
        path: Some(path_text),
        source: "standard_application_directories".into(),
        instance: None,
    })
}

fn running(identity: &UserIdentity, snapshot: &mut AppListSnapshot) -> Result<(), String> {
    let uid: u32 = identity
        .account_id
        .strip_prefix("uid:")
        .and_then(|v| v.parse().ok())
        .ok_or("invalid native account")?;
    let apps = objc2_app_kit::NSWorkspace::sharedWorkspace().runningApplications();
    snapshot
        .sources
        .push("nsworkspace_running_applications".into());
    snapshot.truncated = apps.len() > 4096;
    let mut skipped = 0;
    for app in apps.iter().take(4096) {
        let pid = app.processIdentifier();
        if pid <= 0 || app.isTerminated() {
            continue;
        }
        let Ok(marker) = pab_os_control::process_identity(pid as u32) else {
            skipped += 1;
            continue;
        };
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of_val(&info) as i32;
        if unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        } != size
            || info.pbi_uid != uid
        {
            skipped += 1;
            continue;
        }
        let entry = AppInfo {
            name: app
                .localizedName()
                .map(|v| v.to_string())
                .unwrap_or_default(),
            app_id: app.bundleIdentifier().map(|v| v.to_string()),
            path: app
                .bundleURL()
                .and_then(|v| v.path())
                .map(|v| v.to_string()),
            source: "nsworkspace_running_applications".into(),
            instance: Some(AppInstance {
                process_id: pid as u32,
                process_identity: marker.clone(),
                account_id: Some(identity.account_id.clone()),
                session_id: identity.session_id.map(|v| v.to_string()),
            }),
        };
        if entry.name.len() > 4096
            || entry.path.as_ref().is_some_and(|v| v.len() > 4096)
            || entry.app_id.as_ref().is_some_and(|v| v.len() > 4096)
            || pab_os_control::process_identity(pid as u32).ok().as_ref() != Some(&marker)
        {
            skipped += 1;
            continue;
        }
        snapshot.apps.push(entry);
    }
    if skipped > 0 {
        snapshot.warnings.push(format!(
            "{skipped} instances disappeared or could not be verified as this user"
        ));
    }
    Ok(())
}
