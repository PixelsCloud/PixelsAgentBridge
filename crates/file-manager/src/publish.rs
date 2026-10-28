//! Publication mechanics; the authenticated adapter owns path locks and hashes.
use std::{io, path::Path};
#[cfg(test)]
use std::fs;

/// Same-directory staging with a component length below common filesystem
/// limits. Short legacy names retain their exact spelling; long names use a
/// lossless native-name digest, never a lossy conversion or truncation.
pub fn staging_path(destination: &Path, marker: &str) -> io::Result<std::path::PathBuf> {
    if marker.is_empty()
        || marker.len() > 72
        || !marker
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid staging marker",
        ));
    }
    let file_name = destination
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing destination name"))?;
    let mut name = std::ffi::OsString::from(".");
    if file_name.len() <= 160 {
        name.push(file_name);
    } else {
        let mut digest = blake3::Hasher::new();
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            digest.update(file_name.as_bytes());
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            for unit in file_name.encode_wide() {
                digest.update(&unit.to_le_bytes());
            }
        }
        name.push(format!("pab-name-{}", digest.finalize().to_hex()));
    }
    name.push(format!(".{marker}.part"));
    Ok(destination.with_file_name(name))
}

#[derive(Debug)]
pub struct PublicationReceipt {
    pub published: bool,
    pub staging_retained: bool,
    pub directory_synced: bool,
}

/// Publish a verified staging file in the same directory without deleting an
/// old destination first. Unsupported filesystems fail instead of copying.
pub fn atomic_publish(
    staging: &Path,
    destination: &Path,
    overwrite: bool,
) -> io::Result<PublicationReceipt> {
    let target=crate::StagingTarget::new(staging,destination)?;
    target.check_destination(overwrite)?;
    let file=target.open(false)?;
    target.publish(file,overwrite)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn physical_tempdir()->io::Result<tempfile::TempDir> {tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir())?)}
    #[test]
    fn staging_and_publication_support_maximum_length_file_names() {
        let root = physical_tempdir().unwrap();
        let destination = root.path().join(format!("{}.bin", "x".repeat(240)));
        let staging = staging_path(&destination, "pab-test").unwrap();
        assert_ne!(
            staging,
            staging_path(
                &root.path().join(format!("{}.bin", "y".repeat(240))),
                "pab-test"
            )
            .unwrap()
        );
        fs::write(&staging, b"full name").unwrap();
        atomic_publish(&staging, &destination, false).unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"full name");
    }
    #[test]
    fn replace_is_atomic_and_no_clobber_never_deletes_the_original() {
        let dir = physical_tempdir().unwrap();
        let destination = dir.path().join("result");
        let stage = dir.path().join(".stage");
        fs::write(&destination, b"original").unwrap();
        fs::write(&stage, b"replacement").unwrap();
        assert_eq!(
            atomic_publish(&stage, &destination, false)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&destination).unwrap(), b"original");
        assert_eq!(fs::read(&stage).unwrap(), b"replacement");
        let receipt = atomic_publish(&stage, &destination, true).unwrap();
        assert!(receipt.published);
        assert!(!receipt.staging_retained);
        assert_eq!(fs::read(&destination).unwrap(), b"replacement");
        assert!(!stage.exists());
    }
    #[test]
    fn concurrent_no_clobber_has_exactly_one_publisher() {
        let dir = physical_tempdir().unwrap();
        let destination = dir.path().join("result");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let workers: Vec<_> = (0u8..8)
            .map(|n| {
                let stage = dir.path().join(format!("stage-{n}"));
                fs::write(&stage, [n]).unwrap();
                let destination = destination.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    atomic_publish(&stage, &destination, false).is_ok()
                })
            })
            .collect();
        assert_eq!(
            workers
                .into_iter()
                .filter_map(|worker| worker.join().ok())
                .filter(|ok| *ok)
                .count(),
            1
        );
        assert_eq!(fs::read(destination).unwrap().len(), 1);
    }
    #[test]
    fn directory_and_cross_directory_publication_fail_without_side_effects() {
        let dir = physical_tempdir().unwrap();
        let stage = dir.path().join("stage");
        let target = dir.path().join("target");
        fs::write(&stage, b"new").unwrap();
        fs::create_dir(&target).unwrap();
        assert!(atomic_publish(&stage, &target, true).is_err());
        assert!(target.is_dir());
        let other = physical_tempdir().unwrap();
        assert!(atomic_publish(&stage, &other.path().join("result"), false).is_err());
        assert_eq!(fs::read(stage).unwrap(), b"new");
    }
}
