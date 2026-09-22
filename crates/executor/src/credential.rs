use std::{fs, path::Path};

use argon2::{Argon2, PasswordHash, PasswordVerifier};
use serde::Deserialize;
use thiserror::Error;
use zeroize::Zeroizing;

const DEVICE_CREDENTIAL_SCHEMA_VERSION: u16 = 1;

#[derive(Clone)]
pub(crate) struct DeviceCredential {
    password_version: u64,
    password_hash: String,
}

impl DeviceCredential {
    pub fn read(path: &Path) -> Result<Self, DeviceCredentialError> {
        let encoded = fs::read(path).map_err(|source| DeviceCredentialError::Read {
            path: path.to_owned(),
            source,
        })?;
        let stored: StoredDeviceCredential =
            serde_json::from_slice(&encoded).map_err(|source| DeviceCredentialError::Json {
                path: path.to_owned(),
                source,
            })?;
        if stored.schema_version != DEVICE_CREDENTIAL_SCHEMA_VERSION {
            return Err(DeviceCredentialError::UnsupportedSchema(
                stored.schema_version,
            ));
        }
        if stored.password_version == 0 {
            return Err(DeviceCredentialError::InvalidPasswordVersion);
        }
        let password_hash = PasswordHash::new(&stored.password_hash)
            .map_err(|_| DeviceCredentialError::InvalidPasswordHash)?;
        if password_hash.algorithm.as_str() != "argon2id" {
            return Err(DeviceCredentialError::UnsupportedPasswordHash);
        }
        Ok(Self {
            password_version: stored.password_version,
            password_hash: stored.password_hash,
        })
    }

    pub const fn password_version(&self) -> u64 {
        self.password_version
    }

    pub async fn verify(&self, password: Zeroizing<String>) -> Result<bool, DeviceCredentialError> {
        let password_hash = self.password_hash.clone();
        tokio::task::spawn_blocking(move || {
            let parsed = PasswordHash::new(&password_hash)
                .map_err(|_| DeviceCredentialError::InvalidPasswordHash)?;
            Ok(Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok())
        })
        .await
        .map_err(DeviceCredentialError::Worker)?
    }
}

#[derive(Deserialize)]
struct StoredDeviceCredential {
    schema_version: u16,
    password_version: u64,
    password_hash: String,
}

#[derive(Debug, Error)]
pub enum DeviceCredentialError {
    #[error("the device credential file {path} could not be read: {source}")]
    Read {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("the device credential file {path} is invalid JSON: {source}")]
    Json {
        path: std::path::PathBuf,
        source: serde_json::Error,
    },
    #[error("device credential schema version {0} is not supported")]
    UnsupportedSchema(u16),
    #[error("device password version must be greater than zero")]
    InvalidPasswordVersion,
    #[error("device password hash is not a valid PHC string")]
    InvalidPasswordHash,
    #[error("device password hash must use Argon2id")]
    UnsupportedPasswordHash,
    #[error("device password verification worker failed: {0}")]
    Worker(tokio::task::JoinError),
}

#[cfg(test)]
mod tests {
    use argon2::{PasswordHasher, password_hash::SaltString};

    use super::*;

    #[tokio::test]
    async fn verifies_a_password_without_storing_plaintext() {
        let salt = SaltString::encode_b64(b"pab-test-salt-01").unwrap();
        let hash = Argon2::default()
            .hash_password(b"correct device password", &salt)
            .unwrap()
            .to_string();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credential.json");
        fs::write(
            &path,
            serde_json::json!({
                "schema_version": 1,
                "password_version": 7,
                "password_hash": hash,
            })
            .to_string(),
        )
        .unwrap();

        let credential = DeviceCredential::read(&path).unwrap();
        assert_eq!(credential.password_version(), 7);
        assert!(
            credential
                .verify(Zeroizing::new("correct device password".to_owned()))
                .await
                .unwrap()
        );
        assert!(
            !credential
                .verify(Zeroizing::new("wrong device password".to_owned()))
                .await
                .unwrap()
        );

        fs::write(
            &path,
            serde_json::json!({
                "schema_version": 1,
                "password_version": 8,
                "password_hash": hash.replacen("$argon2id$", "$argon2i$", 1),
            })
            .to_string(),
        )
        .unwrap();
        assert!(matches!(
            DeviceCredential::read(&path),
            Err(DeviceCredentialError::UnsupportedPasswordHash)
        ));
    }
}
