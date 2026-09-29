use std::{collections::HashMap, future::Future, path::PathBuf, pin::Pin, sync::Mutex};

use pab_protocol::{DeviceId, DeviceRef};
use sqlx::{ConnectOptions, sqlite::SqliteConnectOptions};
use thiserror::Error;
use zeroize::Zeroizing;

pub trait DevicePasswordProvider: Send + Sync + 'static {
    fn load_password<'a>(
        &'a self,
        device_ref: DeviceRef,
    ) -> Pin<Box<dyn Future<Output = Result<Zeroizing<String>, RuntimeCredentialError>> + Send + 'a>>;
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

    pub fn forget_password(&self, device_id: DeviceId) -> Result<(), RuntimeCredentialError> {
        self.passwords
            .lock()
            .map_err(|_| RuntimeCredentialError::Unavailable)?
            .remove(&device_id);
        Ok(())
    }
}

impl DevicePasswordProvider for MemoryDevicePasswordProvider {
    fn load_password<'a>(
        &'a self,
        device_ref: DeviceRef,
    ) -> Pin<Box<dyn Future<Output = Result<Zeroizing<String>, RuntimeCredentialError>> + Send + 'a>>
    {
        Box::pin(async move {
            let password = self
                .passwords
                .lock()
                .map_err(|_| RuntimeCredentialError::Unavailable)?
                .get(&device_ref.device_id)
                .cloned();
            if let Some(password) = password {
                return Ok(password);
            }
            self.fallback
                .as_ref()
                .ok_or(RuntimeCredentialError::Missing)?
                .load_password(device_ref)
                .await
        })
    }
}

#[derive(Debug, Clone)]
pub struct SqliteDevicePasswordProvider {
    path: PathBuf,
}

impl SqliteDevicePasswordProvider {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl DevicePasswordProvider for SqliteDevicePasswordProvider {
    fn load_password<'a>(
        &'a self,
        device_ref: DeviceRef,
    ) -> Pin<Box<dyn Future<Output = Result<Zeroizing<String>, RuntimeCredentialError>> + Send + 'a>>
    {
        Box::pin(async move {
            let options = SqliteConnectOptions::new()
                .filename(&self.path)
                .read_only(true);
            let mut connection = options
                .connect()
                .await
                .map_err(RuntimeCredentialError::Database)?;
            let password: Option<String> =
                sqlx::query_scalar("SELECT password FROM device_credentials WHERE device_id = ?")
                    .bind(device_ref.device_id.to_string())
                    .fetch_optional(&mut connection)
                    .await
                    .map_err(RuntimeCredentialError::Database)?;
            password
                .map(Zeroizing::new)
                .ok_or(RuntimeCredentialError::Missing)
        })
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
    fn load_password<'a>(
        &'a self,
        device_ref: DeviceRef,
    ) -> Pin<Box<dyn Future<Output = Result<Zeroizing<String>, RuntimeCredentialError>> + Send + 'a>>
    {
        let path = self.root.join(format!("{}.password", device_ref.device_id));
        Box::pin(async move {
            FileDevicePasswordProvider::new(path)
                .load_password(device_ref)
                .await
        })
    }
}

impl FileDevicePasswordProvider {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl DevicePasswordProvider for FileDevicePasswordProvider {
    fn load_password<'a>(
        &'a self,
        _device_ref: DeviceRef,
    ) -> Pin<Box<dyn Future<Output = Result<Zeroizing<String>, RuntimeCredentialError>> + Send + 'a>>
    {
        Box::pin(async move {
            let bytes = tokio::fs::read(&self.path).await.map_err(|source| {
                RuntimeCredentialError::File {
                    path: self.path.clone(),
                    source,
                }
            })?;
            let mut password =
                String::from_utf8(bytes).map_err(|_| RuntimeCredentialError::Encoding)?;
            let new_length = password.trim_end_matches(['\r', '\n']).len();
            password.truncate(new_length);
            if password.is_empty() {
                return Err(RuntimeCredentialError::Empty);
            }
            Ok(Zeroizing::new(password))
        })
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
    #[error("device password database is unavailable: {0}")]
    Database(sqlx::Error),
}
