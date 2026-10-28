//! Process-wide target reservations. Kept separate from attempt staging
//! locks: two attempts use different staging names but can have one destination.
//! These are cooperative locks on this machine, not filesystem authorization or
//! protection against arbitrary programs modifying the directory.
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use pab_os_control::filesystem::{ProcessFileLock, file_object_identity, fold_file_name};
use std::{collections::HashMap, ffi::OsStr, fs::File, io, sync::Mutex};

pub(crate) struct TargetReservation {
    _name: ProcessFileLock,
    objects: Mutex<HashMap<String, ProcessFileLock>>,
}
impl TargetReservation {
    pub(crate) fn acquire(directory: &Dir, name: &OsStr) -> io::Result<Self> {
        let identity = file_object_identity(&directory.try_clone()?.into_std_file())?;
        let mut key = blake3::Hasher::new();
        key.update(b"name\0");
        key.update(identity.as_bytes());
        key.update(&[0]);
        for unit in fold_file_name(name) {
            key.update(&unit.to_le_bytes());
        }
        let guard = acquire(&key.finalize().to_hex())?;
        let result = Self {
            _name: guard,
            objects: Mutex::new(HashMap::new()),
        };
        result.reserve_destination(directory, name)?;
        Ok(result)
    }
    pub(crate) fn reserve_file(&self, file: &File) -> io::Result<()> {
        let identity = file_object_identity(file)?;
        let mut objects = self
            .objects
            .lock()
            .map_err(|_| io::Error::other("target reservation unavailable"))?;
        if !objects.contains_key(&identity) {
            let key = blake3::hash(format!("object\0{identity}").as_bytes());
            objects.insert(identity, acquire(&key.to_hex())?);
        }
        Ok(())
    }
    pub(crate) fn reserve_destination(&self, directory: &Dir, name: &OsStr) -> io::Result<()> {
        // Attribute-only inspection does not hold up our atomic replacement.
        // It still opens the actual entry, without following final links.
        let mut options = OpenOptions::new();
        #[cfg(windows)]
        options
            .access_mode(0x80)
            .share_mode(1 | 2 | 4)
            .custom_flags(0x0220_0000)
            .follow(FollowSymlinks::No);
        #[cfg(any(target_os="linux",target_os="android"))]
        options
            .read(true)
            .custom_flags(pab_os_control::filesystem::METADATA_OPEN_FLAGS)
            .follow(FollowSymlinks::No);
        match directory.open_with(name, &options) {
            Ok(file) => self.reserve_file(&file.into_std()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}
fn acquire(key: &str) -> io::Result<ProcessFileLock> {
    ProcessFileLock::try_acquire(key)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::WouldBlock,
            "destination is reserved by another file transfer",
        )
    })
}
