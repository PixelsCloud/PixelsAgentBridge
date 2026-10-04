use super::filesystem::FileError;
use pab_protocol::{FileMetadata, MAX_TEXT_FILE_BYTES};
use sha2::{Digest, Sha256};
use std::{
    fs::Metadata,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};
use tokio::{fs, io::AsyncReadExt};

pub(super) fn io_error(phase: &str, error: std::io::Error) -> FileError {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => "not_found",
        std::io::ErrorKind::PermissionDenied => "access_denied",
        std::io::ErrorKind::AlreadyExists => "already_exists",
        _ => "io_error",
    };
    FileError::new(code, phase, error.to_string())
}

pub(super) fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(super) fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub(super) fn metadata(value: &Metadata) -> FileMetadata {
    #[cfg(unix)]
    let unix_mode = {
        use std::os::unix::fs::PermissionsExt;
        Some(value.permissions().mode())
    };
    #[cfg(not(unix))]
    let unix_mode = None;
    FileMetadata {
        kind: if is_link(value) {
            "symlink"
        } else if value.is_file() {
            "file"
        } else if value.is_dir() {
            "directory"
        } else {
            "other"
        }
        .to_owned(),
        size: value.len(),
        modified_at_unix_ms: value
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .and_then(|time| i64::try_from(time.as_millis()).ok()),
        readonly: value.permissions().readonly(),
        unix_mode,
        is_link: is_link(value),
        link_target: None,
        sha256: None,
        encoding: None,
        newline: None,
    }
}

pub(super) fn validate_path(path: &Path) -> Result<(), FileError> {
    if !path.is_absolute() {
        return Err(FileError::new(
            "invalid_path",
            "validate",
            "target path must be absolute",
        ));
    }
    for part in path.components() {
        if part == Component::ParentDir {
            return Err(FileError::new(
                "invalid_path",
                "validate",
                "parent traversal is not supported; use a normalized absolute path",
            ));
        }
        #[cfg(windows)]
        if let Component::Normal(name) = part {
            let name = name.to_string_lossy();
            let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
            if name.contains(':')
                || name.ends_with([' ', '.'])
                || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || (stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && stem.as_bytes()[3].is_ascii_digit())
            {
                return Err(FileError::new(
                    "invalid_path",
                    "validate",
                    "device names, alternate streams and ambiguous Windows names are unsupported",
                ));
            }
        }
    }
    Ok(())
}

pub(super) async fn no_links(path: &Path, missing_final: bool) -> Result<(), FileError> {
    validate_path(path)?;
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part);
        let value = match fs::symlink_metadata(&current).await {
            Ok(value) => value,
            Err(error)
                if missing_final
                    && current == path
                    && error.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(());
            }
            Err(error) => return Err(io_error("inspect_path", error)),
        };
        #[cfg(target_os = "macos")]
        if macos_system_alias(&current, &value).is_some() {
            continue;
        }
        if is_link(&value) {
            return Err(FileError::new(
                "link_not_supported",
                "inspect_path",
                "text tools do not follow symlinks or reparse points, including parent directories",
            ));
        }
    }
    Ok(())
}

// macOS ships these root-owned aliases on its protected system volume. Accept
// only these exact links and destinations; all user-created links still fail.
#[cfg(target_os = "macos")]
pub(super) fn macos_system_alias(path: &Path, metadata: &Metadata) -> Option<Metadata> {
    use std::os::unix::fs::MetadataExt;
    let destination = match path.to_str()? {
        "/var" => "private/var",
        "/tmp" => "private/tmp",
        "/etc" => "private/etc",
        _ => return None,
    };
    if !metadata.file_type().is_symlink()
        || metadata.uid() != 0
        || std::fs::read_link(path).ok()? != Path::new(destination)
    {
        return None;
    }
    let target = std::fs::symlink_metadata(Path::new("/").join(destination)).ok()?;
    (target.is_dir() && target.uid() == 0).then_some(target)
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;
    #[tokio::test]
    async fn system_temp_alias_works_but_user_links_are_rejected() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let file = dir.path().join("text");
        std::fs::write(&file, "data").unwrap();
        no_links(&file, false).await.unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(no_links(&link, false).await.is_err());
    }
}

pub(super) async fn load(path: &Path) -> Result<(Vec<u8>, Metadata), FileError> {
    load_limited(path, MAX_TEXT_FILE_BYTES).await
}

pub(super) async fn load_limited(
    path: &Path,
    limit: usize,
) -> Result<(Vec<u8>, Metadata), FileError> {
    no_links(path, false).await?;
    let inspected = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error("stat", error))?;
    if !inspected.is_file() {
        return Err(FileError::new(
            "not_regular_file",
            "open",
            "text tools require an ordinary file",
        ));
    }
    let mut file = fs::File::open(path)
        .await
        .map_err(|error| io_error("open", error))?;
    let before = file
        .metadata()
        .await
        .map_err(|error| io_error("stat", error))?;
    if !before.is_file() {
        return Err(FileError::new(
            "not_regular_file",
            "open",
            "text tools require an ordinary file",
        ));
    }
    if before.len() > limit as u64 {
        return Err(FileError::new(
            "file_too_large",
            "read",
            "text file exceeds the requested size budget; use file transfer",
        ));
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    (&mut file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| io_error("read", error))?;
    let after = file
        .metadata()
        .await
        .map_err(|error| io_error("stat", error))?;
    if bytes.len() > limit
        || bytes.len() as u64 != before.len()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return Err(FileError::new(
            "version_conflict",
            "read",
            "file changed while being read; retry from a fresh version",
        ));
    }
    Ok((bytes, after))
}

pub(super) fn check_hash(bytes: &[u8], expected: Option<&str>) -> Result<(), FileError> {
    if expected.is_some_and(|expected| expected != digest(bytes)) {
        return Err(FileError::new(
            "version_conflict",
            "validate_version",
            "file SHA-256 differs from expected_hash; read the current version first",
        ));
    }
    Ok(())
}

pub(super) struct Staging(pub(super) PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
