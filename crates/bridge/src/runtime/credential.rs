use std::{fs, path::PathBuf};

use pab_protocol::DeviceRef;
use thiserror::Error;
use zeroize::Zeroizing;

pub trait DevicePasswordProvider: Send + Sync + 'static {
    fn load_password(
        &self,
        device_ref: DeviceRef,
    ) -> Result<Zeroizing<String>, RuntimeCredentialError>;
}

#[derive(Debug, Clone)]
pub struct FileDevicePasswordProvider {
    path: PathBuf,
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
}
