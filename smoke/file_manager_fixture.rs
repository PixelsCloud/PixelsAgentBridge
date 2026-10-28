//! Cleanup shared by owned file-manager native fixtures.
pub async fn cleanup_fixture(dir: tempfile::TempDir) -> std::io::Result<()> {
    // A Windows indexer can briefly retain a file after all our SQLite handles
    // close. Retry only this exact owned root; never report pass before cleanup.
    let root = std::fs::canonicalize(dir.path())?;
    let parent = root
        .parent()
        .ok_or_else(|| std::io::Error::other("fixture has no parent"))?
        .to_owned();
    let kept = dir.keep();
    if std::fs::canonicalize(&kept)? != root {
        return Err(std::io::Error::other("fixture identity changed"));
    }
    for attempt in 0..50 {
        match std::fs::symlink_metadata(&root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
            Ok(meta) => {
                #[cfg(windows)]
                let link = {
                    use std::os::windows::fs::MetadataExt;
                    meta.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let link = meta.file_type().is_symlink();
                if !meta.is_dir()
                    || link
                    || std::fs::canonicalize(&root)?.parent() != Some(parent.as_path())
                {
                    return Err(std::io::Error::other("refusing changed fixture root"));
                }
            }
        }
        match std::fs::remove_dir_all(&root) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if attempt == 49 => return Err(error),
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
        }
    }
    unreachable!()
}
