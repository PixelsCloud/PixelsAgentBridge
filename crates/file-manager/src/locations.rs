use crate::{FileManagerError, encode_path};
use pab_protocol::{FileManagerLocations, FileManagerRoot, PathStyle};
#[cfg(unix)]
use std::path::PathBuf;

/// Observe the account actually running this adapter. Never borrow the service
/// account's home when called inside a selected user's worker.
pub fn locations() -> Result<FileManagerLocations, FileManagerError> {
    let identity = pab_os_control::execution::current_identity()?;
    #[cfg(windows)]
    let roots = pab_os_control::filesystem::logical_drive_roots()?;
    #[cfg(unix)]
    let roots = unix_roots();
    // Native account data is trusted location discovery. Publish its physical
    // home (e.g. macOS /private/var/root) so later no-follow IO needs no alias.
    let home = identity.home.is_dir().then(||std::fs::canonicalize(&identity.home).map(|home|encode_path(&home))).transpose()?;
    let mut locations: Vec<FileManagerRoot> = roots.into_iter().take(64).map(|path| FileManagerRoot {
        name: encode_path(&path).display_label(), path: encode_path(&path),
    }).collect();
    // Resolve native known folders in this adapter's actual user context,
    // including redirected Windows folders and XDG user directories.
    for (name, folder) in [("Desktop", dirs::desktop_dir()), ("Downloads", dirs::download_dir()),
        ("Documents", dirs::document_dir()), ("Pictures", dirs::picture_dir()),
        ("Music", dirs::audio_dir()), ("Videos", dirs::video_dir())] {
        if locations.len() >= 64 { break; }
        if let Some(folder) = folder.filter(|path| path.is_dir()) {
            if let Ok(path) = std::fs::canonicalize(folder) {
                locations.push(FileManagerRoot { name: name.into(), path: encode_path(&path) });
            }
        }
    }
    Ok(FileManagerLocations {
        filesystem_scope: Some(crate::filesystem_scope()?),
        path_style: if cfg!(windows) {
            PathStyle::Windows
        } else {
            PathStyle::Posix
        },
        home,
        roots: locations,
    })
}

#[cfg(unix)]
fn unix_roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/")];
    #[cfg(target_os = "linux")]
    {
        use std::{io::Read, os::unix::ffi::OsStringExt};
        if let Ok(file) = std::fs::File::open("/proc/self/mountinfo") {
            let mut content = Vec::new();
            if file.take(1024 * 1024).read_to_end(&mut content).is_ok() {
                for line in content.split(|byte| *byte == b'\n') {
                    let fields: Vec<_> = line.split(|byte| *byte == b' ').collect();
                    let Some(field) = fields.get(4) else { continue };
                    let bytes = unescape_mount(field);
                    let path = PathBuf::from(std::ffi::OsString::from_vec(bytes));
                    if !path.is_absolute()
                        || roots.contains(&path)
                        || ["/proc", "/sys", "/dev", "/run/user", "/run/credentials"]
                            .iter()
                            .any(|prefix| path.starts_with(prefix))
                    {
                        continue;
                    }
                    if encode_path(&path).validate().is_ok() {
                        roots.push(path);
                    }
                    if roots.len() >= 64 {
                        break;
                    }
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    if let Ok(volumes) = std::fs::read_dir("/Volumes") {
        for entry in volumes.filter_map(Result::ok).take(63) {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                roots.push(entry.path());
            }
        }
    }
    roots
}

#[cfg(target_os = "linux")]
fn unescape_mount(value: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(value.len());
    let mut offset = 0;
    while offset < value.len() {
        if value[offset] == b'\\'
            && offset + 3 < value.len()
            && value[offset + 1..offset + 4]
                .iter()
                .all(|byte| (b'0'..=b'7').contains(byte))
        {
            let number = (value[offset + 1] - b'0') as u16 * 64
                + (value[offset + 2] - b'0') as u16 * 8
                + (value[offset + 3] - b'0') as u16;
            if number <= 255 {
                result.push(number as u8);
                offset += 4;
                continue;
            }
        }
        result.push(value[offset]);
        offset += 1;
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn locations_describe_the_current_native_identity() {
        let locations = super::locations().unwrap();
        assert!(!locations.roots.is_empty());
        assert!(locations.roots.len() <= 64);
        for root in locations.roots {
            assert!(crate::decode_path(&root.path).unwrap().is_absolute());
        }
        if let Some(home) = locations.home {
            assert_eq!(crate::decode_path(&home).unwrap(), std::fs::canonicalize(pab_os_control::execution::current_identity().unwrap().home).unwrap());
        }
    }
}
