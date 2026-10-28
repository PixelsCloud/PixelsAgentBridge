use crate::{FileManagerError, decode_path};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::OpenOptionsExt;
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use pab_protocol::{FilePathIdentity, ManagedFileKind, NativeFilePath};
use std::path::{Component, Path, PathBuf};

pub fn filesystem_scope() -> Result<String, FileManagerError> {
    let mut hash = blake3::Hasher::new();
    hash.update(b"PixelsAgentBridge.FilesystemScope.v1\0");
    hash.update(std::env::consts::OS.as_bytes());
    hash.update(&pab_os_control::filesystem::filesystem_boot_scope()?);
    Ok(hash.finalize().to_hex().to_string())
}
/// Observe objects relative to retained no-follow directories. Missing suffixes
/// remain absent; permission errors and links are never treated as absence.
pub fn inspect_path_identity(path: &NativeFilePath) -> Result<FilePathIdentity, FileManagerError> {
    let path = decode_path(path)?;
    let mut root = PathBuf::new();
    let mut names = Vec::new();
    for part in path.components() {
        match part {
            Component::Prefix(_) | Component::RootDir if names.is_empty() => {
                root.push(part.as_os_str())
            }
            Component::Normal(name) => {
                crate::decode_component(&crate::encode_path(Path::new(name)))?;
                names.push(name);
            }
            _ => {
                return Err(FileManagerError::Invalid(
                    "relative components are not allowed",
                ));
            }
        }
    }
    if names.len() > 256 {
        return Err(FileManagerError::Capacity);
    }
    let mut directory = Dir::open_ambient_dir(&root, ambient_authority())?;
    let mut result = FilePathIdentity {
        filesystem_scope: filesystem_scope()?,
        object: None,
        kind: None,
        ancestors: Vec::new(),
    };
    let identify = |directory: &Dir| {
        pab_os_control::filesystem::file_object_identity(&directory.try_clone()?.into_std_file())
    };
    if names.is_empty() {
        result.object = Some(identify(&directory)?);
        result.kind = Some(ManagedFileKind::Directory);
        return Ok(result);
    }
    for (index, name) in names.iter().enumerate() {
        result.ancestors.push(identify(&directory)?);
        if index + 1 < names.len() {
            match directory.open_dir_nofollow(name) {
                Ok(next) => directory = next,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(result),
                Err(error) => return Err(error.into()),
            }
        } else {
            let before = match directory.symlink_metadata(name) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(result),
                Err(error) => return Err(error.into()),
            };
            if !before.is_file() && !before.is_dir() {
                return Err(FileManagerError::Invalid(
                    "only ordinary files and directories have copy identities",
                ));
            }
            let mut options = OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            #[cfg(unix)]
            options.custom_flags(pab_os_control::filesystem::METADATA_OPEN_FLAGS);
            #[cfg(windows)]
            options
                .access_mode(0x80)
                .share_mode(1 | 2 | 4)
                .custom_flags(0x0220_0000);
            let file = match directory.open_with(name, &options) {
                Ok(file) => file.into_std(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(result),
                Err(error) => return Err(error.into()),
            };
            let metadata = file.metadata()?;
            #[cfg(windows)]
            let reparse = {
                use std::os::windows::fs::MetadataExt;
                metadata.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let reparse = false;
            if reparse || metadata.file_type().is_symlink() {
                return Err(FileManagerError::Invalid(
                    "linked filesystem objects are not supported",
                ));
            }
            if !metadata.is_file() && !metadata.is_dir() {
                return Err(FileManagerError::Invalid(
                    "filesystem object type changed during identity observation",
                ));
            }
            result.kind = Some(if metadata.is_dir() {
                ManagedFileKind::Directory
            } else if metadata.is_file() {
                ManagedFileKind::File
            } else {
                ManagedFileKind::Other
            });
            result.object = Some(pab_os_control::filesystem::file_object_identity(&file)?);
        }
    }
    result.validate().map_err(FileManagerError::Invalid)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn observe(path: &Path) -> FilePathIdentity {
        inspect_path_identity(&crate::encode_path(path)).unwrap()
    }
    #[test]
    fn same_objects_hardlinks_and_directory_descendants_are_rejected_without_writes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("source");
        std::fs::create_dir_all(source.join("inside")).unwrap();
        let file = source.join("file");
        std::fs::write(&file, b"original").unwrap();
        let alias = root.join("alias");
        std::fs::hard_link(&file, &alias).unwrap();
        let identity = observe(&source);
        assert_eq!(identity.filesystem_scope, filesystem_scope().unwrap());
        assert!(
            identity
                .validate_copy_destination(&observe(&source))
                .is_err()
        );
        assert!(
            identity
                .validate_copy_destination(&observe(&source.join("inside/new/deeper")))
                .is_err()
        );
        assert!(
            observe(&file)
                .validate_copy_destination(&observe(&alias))
                .is_err()
        );
        assert!(
            identity
                .validate_copy_destination(&observe(&root.join("sibling/new")))
                .is_ok()
        );
        #[cfg(windows)]
        assert!(
            observe(&file)
                .validate_copy_destination(&observe(&source.join("FILE")))
                .is_err()
        );
        assert!(!source.join("inside/new").exists());
        assert!(!root.join("sibling").exists());
        assert_eq!(std::fs::read(file).unwrap(), b"original");
    }
    #[test]
    fn identity_inspection_rejects_linked_ancestors_and_relative_components() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let original = root.join("original");
        std::fs::create_dir(&original).unwrap();
        let linked = root.join("linked");
        #[cfg(windows)]
        {
            let output = std::process::Command::new("cmd.exe")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&linked)
                .arg(&original)
                .output()
                .unwrap();
            assert!(output.status.success());
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&original, &linked).unwrap();
        assert!(inspect_path_identity(&crate::encode_path(&linked.join("absent"))).is_err());
        assert!(inspect_path_identity(&crate::encode_path(&linked)).is_err());
        // Verbatim Windows PathBuf::join normalizes parent components. Supply
        // the original bytes so this really tests rejection, not a fixed path.
        let mut relative = original.as_os_str().to_os_string();
        relative.push(if cfg!(windows) {
            "\\..\\absent"
        } else {
            "/../absent"
        });
        assert!(inspect_path_identity(&crate::encode_path(Path::new(&relative))).is_err());
        #[cfg(windows)]
        std::fs::remove_dir(linked).unwrap();
    }
}
