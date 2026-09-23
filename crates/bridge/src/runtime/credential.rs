use std::{collections::HashMap, fs, path::PathBuf, sync::Mutex};

use pab_protocol::{DeviceId, DeviceRef};
use thiserror::Error;
use zeroize::Zeroizing;

pub trait DevicePasswordProvider: Send + Sync + 'static {
    fn load_password(
        &self,
        device_ref: DeviceRef,
    ) -> Result<Zeroizing<String>, RuntimeCredentialError>;
}

#[derive(Default)]
pub struct MemoryDevicePasswordProvider {
    passwords: Mutex<HashMap<DeviceId, Zeroizing<String>>>,
    fallback: Option<Box<dyn DevicePasswordProvider>>,
}

impl MemoryDevicePasswordProvider {
    pub fn new(fallback: Option<Box<dyn DevicePasswordProvider>>) -> Self {
        Self {
            passwords: Mutex::new(HashMap::new()),
            fallback,
        }
    }

    pub fn set_password(
        &self,
        device_id: DeviceId,
        password: String,
    ) -> Result<(), RuntimeCredentialError> {
        if password.is_empty() {
            return Err(RuntimeCredentialError::Empty);
        }
        self.passwords
            .lock()
            .map_err(|_| RuntimeCredentialError::Unavailable)?
            .insert(device_id, Zeroizing::new(password));
        Ok(())
    }
}

impl DevicePasswordProvider for MemoryDevicePasswordProvider {
    fn load_password(
        &self,
        device_ref: DeviceRef,
    ) -> Result<Zeroizing<String>, RuntimeCredentialError> {
        if let Some(password) = self
            .passwords
            .lock()
            .map_err(|_| RuntimeCredentialError::Unavailable)?
            .get(&device_ref.device_id)
        {
            return Ok(password.clone());
        }
        self.fallback
            .as_ref()
            .ok_or(RuntimeCredentialError::Missing)?
            .load_password(device_ref)
    }
}

#[derive(Debug, Clone)]
pub struct FileDevicePasswordProvider {
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct DirectoryDevicePasswordProvider {
    root: PathBuf,
}

impl DirectoryDevicePasswordProvider {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl DevicePasswordProvider for DirectoryDevicePasswordProvider {
    fn load_password(
        &self,
        device_ref: DeviceRef,
    ) -> Result<Zeroizing<String>, RuntimeCredentialError> {
        FileDevicePasswordProvider::new(
            self.root.join(format!("{}.password", device_ref.device_id)),
        )
        .load_password(device_ref)
    }
}

impl FileDevicePasswordProvider {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl DevicePasswordProvider for FileDevicePasswordProvider {
    fn load_password(
        &self,
        _device_ref: DeviceRef,
    ) -> Result<Zeroizing<String>, RuntimeCredentialError> {
        let bytes = fs::read(&self.path).map_err(|source| RuntimeCredentialError::File {
            path: self.path.clone(),
            source,
        })?;
        let mut password =
            String::from_utf8(bytes).map_err(|_| RuntimeCredentialError::Encoding)?;
        let new_length = password.trim_end_matches(['\r', '\n']).len();
        password.truncate(new_length);
        if password.is_empty() {
            return Err(RuntimeCredentialError::Empty);
        }
        Ok(Zeroizing::new(password))
    }
}

#[derive(Debug, Error)]
pub enum RuntimeCredentialError {
    #[error("device password file {path} could not be read: {source}")]
    File {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("device password must contain UTF-8 text")]
    Encoding,
    #[error("device password is empty")]
    Empty,
    #[error("device password has not been entered")]
    Missing,
    #[error("device password store is unavailable")]
    Unavailable,
}
