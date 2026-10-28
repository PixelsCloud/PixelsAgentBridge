//! Single-object operations anchored to open directory handles. The endpoint
//! supplies authenticated identity, durable acceptance and path coordination.
use crate::{FileManagerError, decode_path, encode_path};
use cap_fs_ext::DirExt;
use cap_std::{ambient_authority, fs::Dir};
use pab_protocol::NativeFilePath;
use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

/// Resolve an explicitly selected absolute directory without following a link
/// in any component. Subsequent operations use the retained directory handle.
pub(crate) fn open_directory(path: &Path) -> Result<Dir, FileManagerError> {
    if !path.is_absolute() {
        return Err(FileManagerError::Invalid("expected absolute directory"));
    }
    let mut root = PathBuf::new();
    let mut components = path.components().peekable();
    while components
        .peek()
        .is_some_and(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
    {
        root.push(components.next().unwrap().as_os_str());
    }
    let mut directory = Dir::open_ambient_dir(&root, ambient_authority())?;
    for component in components {
        match component {
            Component::Normal(name) => {
                directory = directory.open_dir_nofollow(name)?;
            }
            _ => {
                return Err(FileManagerError::Invalid(
                    "relative directory components are not allowed",
                ));
            }
        }
    }
    Ok(directory)
}

fn child(path: &Path) -> Result<(&Path, &OsStr), FileManagerError> {
    // For mutations reject roots, trailing dot/parent components and ADS before
    // opening anything. Lossy display strings never identify the object.
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(FileManagerError::Invalid(
            "relative path components are not allowed",
        ));
    }
    let name = path
        .file_name()
        .ok_or(FileManagerError::Invalid("cannot mutate a filesystem root"))?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        if name.encode_wide().any(|c| c == b':' as u16) {
            return Err(FileManagerError::Invalid(
                "alternate streams are not supported",
            ));
        }
    }
    Ok((
        path.parent()
            .ok_or(FileManagerError::Invalid("missing parent"))?,
        name,
    ))
}

pub fn validate_new_name(name: &str) -> Result<(), FileManagerError> {
    if name.is_empty()
        || name.len() > 1024
        || name == "."
        || name == ".."
        || name.contains(['\0', '/', '\\'])
    {
        return Err(FileManagerError::Invalid("invalid filename"));
    }
    #[cfg(windows)]
    {
        let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|n| {
                matches!(
                    n,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            });
        if reserved
            || name.ends_with(['.', ' '])
            || name.chars().any(|c| c < ' ' || "<>:\"|?*".contains(c))
        {
            return Err(FileManagerError::Invalid(
                "filename is not representable on Windows",
            ));
        }
    }
    Ok(())
}

pub fn decode_component(name: &NativeFilePath) -> Result<std::ffi::OsString, FileManagerError> {
    name.validate_component().map_err(FileManagerError::Invalid)?;
    let name = match name {
        NativeFilePath::Utf8(name) => std::ffi::OsString::from(name),
        #[cfg(unix)]
        NativeFilePath::UnixBytes(name) => { use std::os::unix::ffi::OsStringExt; std::ffi::OsString::from_vec(name.clone()) },
        #[cfg(windows)]
        NativeFilePath::WindowsWide(name) => { use std::os::windows::ffi::OsStringExt; std::ffi::OsString::from_wide(name) },
        _ => return Err(FileManagerError::Invalid("native filename belongs to another platform")),
    };
    let mut parts = Path::new(&name).components();
    if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() { return Err(FileManagerError::Invalid("expected a single native filename")); }
    #[cfg(windows)]
    {
        // Lossy text is used only to reject reserved device names. The actual
        // operation always retains the original UTF-16 OsString above.
        validate_new_name(&name.to_string_lossy())?;
    }
    Ok(name)
}

pub fn create_directory_native(parent: &NativeFilePath, name: &NativeFilePath) -> Result<NativeFilePath, FileManagerError> {
    let name = decode_component(name)?;
    let parent = decode_path(parent)?;
    let target = encode_path(&parent.join(&name));
    target.validate().map_err(FileManagerError::Invalid)?;
    open_directory(&parent)?.create_dir(&name)?;
    Ok(target)
}

pub fn rename_native(path: &NativeFilePath, name: &NativeFilePath) -> Result<NativeFilePath, FileManagerError> {
    let name = decode_component(name)?;
    let path = decode_path(path)?;
    let (parent, source) = child(&path)?;
    if source == name { return Err(FileManagerError::Invalid("source and destination are the same")); }
    let target = encode_path(&parent.join(&name));
    target.validate().map_err(FileManagerError::Invalid)?;
    let dir = open_directory(parent)?;
    #[cfg(windows)]
    {
        use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
        use cap_std::fs::{OpenOptions, OpenOptionsExt};
        let mut options = OpenOptions::new();
        options.access_mode(0x0001_0000 | 0x0010_0000).share_mode(1|2).custom_flags(0x0200_0000 | 0x0020_0000).follow(FollowSymlinks::No);
        let file = dir.open_with(source, &options)?.into_std();
        pab_os_control::filesystem::rename_open_no_replace(&file, &dir.try_clone()?.into_std_file(), &name)?;
    }
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    pab_os_control::filesystem::rename_relative_no_replace(&dir.try_clone()?.into_std_file(), source, &name)?;
    Ok(target)
}

pub fn create_directory(
    parent: &NativeFilePath,
    name: &str,
) -> Result<NativeFilePath, FileManagerError> {
    validate_new_name(name)?;
    let parent = decode_path(parent)?;
    let dir = open_directory(&parent)?;
    dir.create_dir(name)?; // exclusive; existing objects are never accepted
    Ok(encode_path(&parent.join(name)))
}

pub fn rename(path: &NativeFilePath, name: &str) -> Result<NativeFilePath, FileManagerError> {
    validate_new_name(name)?;
    let path = decode_path(path)?;
    let (parent, source) = child(&path)?;
    if source == OsStr::new(name) {
        return Err(FileManagerError::Invalid(
            "source and destination are the same",
        ));
    }
    let dir = open_directory(parent)?;
    #[cfg(windows)]
    {
        use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
        use cap_std::fs::{OpenOptions, OpenOptionsExt};
        let mut options = OpenOptions::new();
        options.access_mode(0x0001_0000 | 0x0010_0000); // DELETE | SYNCHRONIZE, no content write access
        options.share_mode(1 | 2); // retain the selected object against a concurrent rename/delete
        options.custom_flags(0x0200_0000 | 0x0020_0000); // BACKUP_SEMANTICS | OPEN_REPARSE_POINT
        options.follow(FollowSymlinks::No);
        let file = dir.open_with(source, &options)?.into_std();
        pab_os_control::filesystem::rename_open_no_replace(
            &file,
            &dir.try_clone()?.into_std_file(),
            OsStr::new(name),
        )?;
    }
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    pab_os_control::filesystem::rename_relative_no_replace(
        &dir.try_clone()?.into_std_file(),
        source,
        OsStr::new(name),
    )?;
    #[cfg(not(any(
        windows,
        target_os = "linux",
        target_os = "android",
        target_os = "macos"
    )))]
    return Err(FileManagerError::Invalid(
        "atomic rename unsupported on this platform",
    ));
    Ok(encode_path(&parent.join(name)))
}

/// Permanently delete only the selected object. Directory traversal is bounded
/// by the open capability and never follows directory links to outside data.
pub fn delete(path: &NativeFilePath, recursive: bool) -> Result<(), FileManagerError> {
    let path = decode_path(path)?;
    let (parent, name) = child(&path)?;
    let dir = open_directory(parent)?;
    let meta = dir.symlink_metadata(name)?;
    if meta.file_type().is_symlink() {
        dir.remove_file_or_symlink(name)?;
    } else if meta.is_dir() {
        if recursive {
            dir.remove_dir_all(name)?;
        } else {
            dir.remove_dir(name)?;
        }
    } else {
        dir.remove_file(name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_directory_names_remain_lossless() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        #[cfg(windows)]
        let name = NativeFilePath::WindowsWide(vec![0xd800, 0x61]);
        #[cfg(unix)]
        let name = NativeFilePath::UnixBytes(vec![0xff, b'a', b'\\']);
        let created = create_directory_native(&encode_path(&root), &name).unwrap();
        assert!(decode_path(&created).unwrap().is_dir());
        let renamed = rename_native(&created, &"renamed".into()).unwrap();
        assert_eq!(decode_path(&renamed).unwrap(), root.join("renamed"));
        assert!(!decode_path(&created).unwrap().exists());
        delete(&renamed, false).unwrap();
    }
    #[test]
    fn mutations_are_exclusive_and_roots_are_protected() {
        let root = tempfile::tempdir().unwrap();
        let first = create_directory(&encode_path(root.path()), "first").unwrap();
        assert!(create_directory(&encode_path(root.path()), "first").is_err());
        std::fs::write(root.path().join("occupied"), b"original").unwrap();
        assert!(rename(&first, "occupied").is_err());
        assert_eq!(
            std::fs::read(root.path().join("occupied")).unwrap(),
            b"original"
        );
        let renamed = rename(&first, "renamed").unwrap();
        std::fs::write(root.path().join("renamed/child"), b"child").unwrap();
        assert!(delete(&renamed, false).is_err());
        delete(&renamed, true).unwrap();
        assert!(!root.path().join("renamed").exists());
        assert!(child(root.path().ancestors().last().unwrap()).is_err());
        for name in [".", "..", "a/b", "a\\b", ""] {
            assert!(validate_new_name(name).is_err());
        }
    }

    #[test]
    fn recursive_delete_preserves_the_target_of_an_external_link() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("keep"), b"untouched").unwrap();
        let selected = root.path().join("selected");
        std::fs::create_dir(&selected).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), selected.join("external")).unwrap();
        #[cfg(windows)]
        {
            // Junction creation requires no symlink privilege. The command is
            // fixed; arguments are generated only by tempfile, never user input.
            let result = std::process::Command::new("cmd.exe")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(selected.join("external"))
                .arg(outside.path())
                .output()
                .unwrap();
            assert!(result.status.success(), "{:?}", result);
        }
        assert!(open_directory(&selected.join("external")).is_err());
        delete(&encode_path(&selected), true).unwrap();
        assert_eq!(
            std::fs::read(outside.path().join("keep")).unwrap(),
            b"untouched"
        );
    }
}
