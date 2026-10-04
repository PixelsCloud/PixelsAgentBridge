use std::path::Path;

use argon2::{Argon2, PasswordHash, PasswordVerifier};
use thiserror::Error;
use zeroize::Zeroizing;

#[derive(Clone)]
pub(crate) struct DeviceCredential {
    password_version: u64,
    password_hash: String,
}

impl DeviceCredential {
    pub async fn read(path: &Path) -> Result<Self, DeviceCredentialError> {
        let stored = crate::device_access::load(path).await?;
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

#[derive(Debug, Error)]
pub enum DeviceCredentialError {
    #[error(transparent)]
    Store(#[from] crate::device_access::DeviceAccessError),
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
    async fn verifies_a_password_from_the_device_database() {
        let salt = SaltString::encode_b64(b"pab-test-salt-01").unwrap();
        let hash = Argon2::default()
            .hash_password(b"correct device password", &salt)
            .unwrap()
            .to_string();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credential.json");
        crate::device_access::save(
            &path,
            &crate::device_access::DeviceAccess {
                tenant_id: "tenant".to_owned(),
                device_id: "device".to_owned(),
                device_code: "123456789".to_owned(),
                temporary_password: "correct device password".to_owned(),
                password_version: 7,
                password_hash: hash.clone(),
            },
        )
        .await
        .unwrap();

        let credential = DeviceCredential::read(&path).await.unwrap();
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

        let mut access = crate::device_access::load(&path).await.unwrap();
        access.password_version = 8;
        access.password_hash = hash.replacen("$argon2id$", "$argon2i$", 1);
        crate::device_access::save(&path, &access).await.unwrap();
        assert!(matches!(
            DeviceCredential::read(&path).await,
            Err(DeviceCredentialError::UnsupportedPasswordHash)
        ));
    }
}
