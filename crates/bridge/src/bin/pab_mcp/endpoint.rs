//! A network endpoint belongs to one live MCP process, not to the shared DB.
//! Keep the lease for the process lifetime, including registration retries.
use std::{
    fs::{File, OpenOptions, TryLockError},
    path::{Path, PathBuf},
    sync::Mutex,
};

use pab_agent_core::{
    DataPaths, DataScope, ensure_data_parent, load_or_create_endpoint_secret, read_endpoint_secret,
};

static PROCESS_ENDPOINT: Mutex<Option<EndpointLease>> = Mutex::new(None);

struct EndpointLease {
    secret_path: PathBuf,
    // Never unlink a lock file: another process may already have it open.
    // Closing the handle (also on process termination) releases the OS lock.
    _lock: File,
}

impl EndpointLease {
    fn account(root: &Path, path: &Path) -> Result<Self, String> {
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        let key = read_endpoint_secret(&path).map_err(|e| e.to_string())?;
        // Lock by public identity, even if callers use copies of the key file.
        // Keep locks in user storage; registered keys may be read-only.
        let marker = root
            .join("mcp-endpoints")
            .join(format!("account-{}", key.public()));
        let mut lease = Self::try_reserve(&marker)?.ok_or(
            "This account endpoint is already used by another MCP process; configure a separately registered PAB_ENDPOINT_SECRET_FILE for each concurrent account MCP",
        )?;
        lease.secret_path = path;
        Ok(lease)
    }

    fn guest(root: &Path) -> Result<Self, String> {
        for slot in 0..u32::MAX {
            let secret_path = root.join("mcp-endpoints").join(format!("guest-{slot}.key"));
            if let Some(lease) = Self::try_reserve(&secret_path)? {
                // The slot lock also serializes first-time key creation.
                // Corrupt keys fail explicitly; never silently change identity.
                load_or_create_endpoint_secret(&secret_path).map_err(|e| e.to_string())?;
                return Ok(lease);
            }
        }
        Err("No free MCP endpoint slot".to_owned())
    }

    fn try_reserve(secret_path: &Path) -> Result<Option<Self>, String> {
        let lock_path = secret_path.with_added_extension("lock");
        ensure_data_parent(&lock_path).map_err(|e| e.to_string())?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(&lock_path).map_err(|e| e.to_string())?;
        match lock.try_lock() {
            Ok(()) => Ok(Some(Self {
                secret_path: secret_path.to_owned(),
                _lock: lock,
            })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error.to_string()),
        }
    }
}

pub(super) fn guest_secret_path() -> Result<PathBuf, String> {
    let mut slot = PROCESS_ENDPOINT.lock().map_err(|e| e.to_string())?;
    if slot.is_none() {
        let paths = DataPaths::for_scope(DataScope::User).map_err(|e| e.to_string())?;
        *slot = Some(EndpointLease::guest(paths.root())?);
    }
    Ok(slot.as_ref().unwrap().secret_path.clone())
}

pub(super) fn reserve_account_secret(path: &Path) -> Result<(), String> {
    let mut slot = PROCESS_ENDPOINT.lock().map_err(|e| e.to_string())?;
    if slot.is_none() {
        // Account keys are pre-registered. Do not replace one with an anonymous
        // guest key, or silently change the user's configured identity.
        let paths = DataPaths::for_scope(DataScope::User).map_err(|e| e.to_string())?;
        *slot = Some(EndpointLease::account(paths.root(), path)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pab_agent_core::read_endpoint_secret;

    #[test]
    fn concurrent_slots_are_distinct_and_released_slots_keep_identity() {
        let root = tempfile::tempdir().unwrap();
        let desktop_path = root.path().join("guest-endpoint.key");
        let desktop = load_or_create_endpoint_secret(&desktop_path).unwrap();
        let first = EndpointLease::guest(root.path()).unwrap();
        let first_path = first.secret_path.clone();
        let first_key = read_endpoint_secret(&first_path).unwrap();
        let second = EndpointLease::guest(root.path()).unwrap();
        let second_key = read_endpoint_secret(&second.secret_path).unwrap();
        assert_ne!(first_key.public(), second_key.public());
        assert_ne!(first_key.public(), desktop.public());
        assert_ne!(second_key.public(), desktop.public());
        drop(first);
        let replacement = EndpointLease::guest(root.path()).unwrap();
        assert_eq!(replacement.secret_path, first_path);
        assert_eq!(
            read_endpoint_secret(&replacement.secret_path)
                .unwrap()
                .public(),
            first_key.public()
        );
        assert_eq!(
            read_endpoint_secret(&desktop_path).unwrap().public(),
            desktop.public()
        );
    }

    #[test]
    fn explicit_account_key_cannot_be_reserved_twice() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("account.key");
        load_or_create_endpoint_secret(&path).unwrap();
        let copy = root.path().join("account-copy.key");
        std::fs::copy(&path, &copy).unwrap();
        let first = EndpointLease::account(root.path(), &path).unwrap();
        assert!(EndpointLease::account(root.path(), &path).is_err());
        assert!(EndpointLease::account(root.path(), &copy).is_err());
        drop(first);
        assert!(EndpointLease::account(root.path(), &copy).is_ok());
    }

    #[test]
    fn corrupt_key_is_not_replaced_and_failed_acquisition_releases_lock() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("mcp-endpoints/guest-0.key");
        ensure_data_parent(&path).unwrap();
        std::fs::write(&path, "invalid").unwrap();
        assert!(EndpointLease::guest(root.path()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "invalid");
        assert!(EndpointLease::try_reserve(&path).unwrap().is_some());
    }

    #[test]
    fn storage_errors_do_not_fall_back_to_desktop_key() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("mcp-endpoints"), "not a directory").unwrap();
        assert!(EndpointLease::guest(root.path()).is_err());
        assert!(!root.path().join("guest-endpoint.key").exists());
    }
}
