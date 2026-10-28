//! Native atomic rename without replacing a concurrently created destination.
use std::{io, path::Path};
mod boot_scope;
pub use boot_scope::filesystem_boot_scope;

/// Metadata observation must not wait for a FIFO peer or acquire a terminal.
#[cfg(any(target_os="linux",target_os="android"))]
pub const METADATA_OPEN_FLAGS: i32 = libc::O_PATH | libc::O_NOFOLLOW;
#[cfg(target_os="macos")]
pub const METADATA_OPEN_FLAGS: i32 = libc::O_NONBLOCK | libc::O_NOCTTY;
/// Opening an expected directory must never wait on a replacement FIFO/device.
#[cfg(unix)]
pub const DIRECTORY_OPEN_FLAGS: i32 = libc::O_DIRECTORY | libc::O_NONBLOCK | libc::O_NOCTTY;
#[cfg(unix)]
pub const NONBLOCKING_FILE_OPEN_FLAGS: i32 = libc::O_NONBLOCK | libc::O_NOCTTY;

#[cfg(windows)]
mod process_lock;
#[cfg(windows)]
pub use process_lock::{ProcessFileLock, fold_file_name};

#[cfg(any(target_os="linux",target_os="android"))]
mod process_lock_linux;
#[cfg(any(target_os="linux",target_os="android"))]
pub use process_lock_linux::{ProcessFileLock, fold_file_name};

#[cfg(windows)]
mod directory_create;
#[cfg(windows)]
pub use directory_create::create_directory_at;
#[cfg(unix)]
mod directory_create_unix;
#[cfg(unix)]
pub use directory_create_unix::create_directory_at;
#[cfg(unix)]
pub use directory_create_unix::{create_directory_at_tracked,cleanup_directory_staging_at,verify_private_directory,sync_directory_handle};

/// Identity of the already-open object, independent of its current pathname.
/// This is an observation, not an authorization token. Callers must still open
/// without following links and compare it under the selected native identity.
pub fn file_object_identity(file: &std::fs::File) -> io::Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        let birth = metadata.created().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
        Ok(format!("u:{:x}:{:x}:{}", metadata.dev(), metadata.ino(),
            birth.map(|value| format!("{:x}", value.as_nanos())).unwrap_or_else(|| "unknown".into())))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
        let mut information = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        // The borrowed File keeps its handle live throughout this query.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let information = unsafe { information.assume_init() };
        Ok(format!("w:{:08x}:{:08x}{:08x}:{:08x}{:08x}",
            information.dwVolumeSerialNumber, information.nFileIndexHigh, information.nFileIndexLow,
            information.ftCreationTime.dwHighDateTime, information.ftCreationTime.dwLowDateTime))
    }
}

/// Query the opened object rather than resolving a staging path again.
pub fn file_link_count(file:&std::fs::File)->io::Result<u64> {
    #[cfg(unix)]
    {use std::os::unix::fs::MetadataExt;Ok(file.metadata()?.nlink())}
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION,GetFileInformationByHandle};
        let mut information=std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        if unsafe {GetFileInformationByHandle(file.as_raw_handle(),information.as_mut_ptr())}==0 {return Err(io::Error::last_os_error());}
        Ok(u64::from(unsafe {information.assume_init()}.nNumberOfLinks))
    }
}

/// Mark the retained receiving object for deletion without resolving its name
/// again. The caller must hold DELETE access and keep the handle until done.
#[cfg(windows)]
pub fn delete_open_file(file:&std::fs::File)->io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{FILE_DISPOSITION_INFO,FileDispositionInfo,SetFileInformationByHandle};
    let information=FILE_DISPOSITION_INFO {DeleteFile:true};
    // The borrowed handle stays live; FILE_DISPOSITION_INFO is fixed-sized.
    if unsafe {SetFileInformationByHandle(file.as_raw_handle(),FileDispositionInfo,(&information as *const FILE_DISPOSITION_INFO).cast(),std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32)}==0 {
        Err(io::Error::last_os_error())
    } else {Ok(())}
}

#[cfg(windows)]
pub fn logical_drive_roots() -> io::Result<Vec<std::path::PathBuf>> {
    // This observes the current native logon's DOS drive map. It neither opens
    // every volume nor queries offline network disks.
    let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
    if mask == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((0u8..26)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| std::path::PathBuf::from(format!("{}:\\", char::from(b'A' + bit))))
        .collect())
}

/// Rename an already opened object into an anchored directory. Neither source
/// nor destination is resolved through an ambient absolute path at publication.
#[cfg(windows)]
pub fn rename_open_no_replace(
    source: &std::fs::File,
    directory: &std::fs::File,
    name: &std::ffi::OsStr,
) -> io::Result<()> {
    rename_open(source, directory, name, false)
}

#[cfg(windows)]
pub fn rename_open(
    source: &std::fs::File,
    directory: &std::fs::File,
    name: &std::ffi::OsStr,
    replace: bool,
) -> io::Result<()> {
    use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
    use windows_sys::{
        Wdk::Storage::FileSystem::{
            FILE_RENAME_INFORMATION, FileRenameInformation, NtSetInformationFile,
        },
        Win32::{Foundation::RtlNtStatusToDosError, System::IO::IO_STATUS_BLOCK},
    };
    single_component(name)?;
    let name: Vec<u16> = name.encode_wide().collect();
    let size = std::mem::size_of::<FILE_RENAME_INFORMATION>() + name.len() * 2;
    // usize alignment is sufficient for FILE_RENAME_INFO, including its HANDLE.
    // The allocation includes the complete variable-length UTF-16 payload.
    let mut storage = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    let mut status = IO_STATUS_BLOCK::default();
    // The source has DELETE access; the caller retains both valid file handles
    // throughout the call. Zero ReplaceIfExists forbids overwriting a racer.
    let result = unsafe {
        (*info).Anonymous.ReplaceIfExists = replace;
        (*info).RootDirectory = directory.as_raw_handle();
        (*info).FileNameLength = (name.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(name.as_ptr(), (*info).FileName.as_mut_ptr(), name.len());
        // The Win32 wrapper rejects relative RootDirectory on some Windows
        // versions. Native FileRenameInformation preserves the anchored name.
        NtSetInformationFile(
            source.as_raw_handle(),
            &mut status,
            info.cast(),
            size as u32,
            FileRenameInformation,
        )
    };
    if result < 0 {
        Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ))
    } else {
        Ok(())
    }
}

/// Atomic no-clobber rename relative to a directory descriptor. The final
/// source component is renamed itself; a symlink is never dereferenced.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
pub fn rename_relative_no_replace(
    directory: &std::fs::File,
    source: &std::ffi::OsStr,
    destination: &std::ffi::OsStr,
) -> io::Result<()> {
    rename_between_no_replace(directory, source, directory, destination)
}

/// Publish a single component between two retained directory descriptors.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
pub fn rename_between_no_replace(
    source_directory: &std::fs::File,
    source: &std::ffi::OsStr,
    destination_directory: &std::fs::File,
    destination: &std::ffi::OsStr,
) -> io::Result<()> {
    use std::{
        ffi::CString,
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
    };
    single_component(source)?;
    single_component(destination)?;
    let source = CString::new(source.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in name"))?;
    let destination = CString::new(destination.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in name"))?;
    // No process cwd or ambient parent resolution participates in publication.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let moved = unsafe {
        libc::renameat2(
            source_directory.as_raw_fd(),
            source.as_ptr(),
            destination_directory.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let moved = unsafe {
        libc::renameatx_np(
            source_directory.as_raw_fd(),
            source.as_ptr(),
            destination_directory.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if moved == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn single_component(name: &std::ffi::OsStr) -> io::Result<()> {
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a single normal filename",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        if name.encode_wide().any(|c| c == 0 || c == b':' as u16) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid filename",
            ));
        }
    }
    Ok(())
}

pub fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    if !source.is_absolute() || !destination.is_absolute() || source == destination {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "rename requires distinct absolute paths",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide = |path: &Path| -> io::Result<Vec<u16>> {
            let mut value: Vec<_> = path.as_os_str().encode_wide().collect();
            if value.contains(&0) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "path contains NUL",
                ));
            }
            value.push(0);
            Ok(value)
        };
        let source = wide(source)?;
        let destination = wide(destination)?;
        // Both pointers remain valid and NUL-terminated for the call. Omitting
        // REPLACE_EXISTING and COPY_ALLOWED preserves atomic no-clobber behavior.
        let moved = unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )
        };
        if moved == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let source = CString::new(source.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
        let destination = CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
        // CString owns both paths through this call; these flags never replace
        // the destination. Unsupported kernels/filesystems return their error.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let moved = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        let moved =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
        if moved == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(not(any(
        windows,
        target_os = "linux",
        target_os = "android",
        target_os = "macos"
    )))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "atomic no-replace rename unavailable",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_replace_works_for_files_and_directories_without_deleting_the_target() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("target");
        std::fs::write(&source, b"source").unwrap();
        std::fs::write(&target, b"target").unwrap();
        assert!(rename_no_replace(&source, &target).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"source");
        assert_eq!(std::fs::read(&target).unwrap(), b"target");
        let renamed = root.path().join("renamed");
        rename_no_replace(&source, &renamed).unwrap();
        assert!(!source.exists());
        let folder = root.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        let other = root.path().join("other");
        std::fs::create_dir(&other).unwrap();
        assert!(rename_no_replace(&folder, &other).is_err());
        assert!(folder.is_dir());
        assert!(other.is_dir());
        let moved = root.path().join("moved");
        rename_no_replace(&folder, &moved).unwrap();
        assert!(moved.is_dir());
        assert!(!folder.exists());
    }
}
