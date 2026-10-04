use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
use pab_agent_core::{
    DataPaths, DataScope, OpenRegistrationKind, load_or_create_endpoint_secret,
    register_open_endpoint, tls_connector,
};
use pab_protocol::EndpointRegistrationResult;
use rand_core::{OsRng, RngCore};
use thiserror::Error;

use crate::{ExecutorConfig, ExecutorConfigError, credential::DeviceCredential};

pub async fn bootstrapped_config() -> Result<ExecutorConfig, BootstrapError> {
    if env::var_os("PAB_DEVICE_ID").is_some() {
        return ExecutorConfig::from_env().map_err(Into::into);
    }
    let paths = DataPaths::for_scope(DataScope::Machine)?;
    let secret_path = env::var_os("PAB_ENDPOINT_SECRET_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.executor_endpoint_secret());
    let secret = load_or_create_endpoint_secret(&secret_path)?;
    let control_url = required("PAB_CONTROL_URL")?;
    let name = env::var("PAB_DEVICE_NAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| env::var("COMPUTERNAME").ok())
        .or_else(|| env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "Remote device".to_owned());
    let ca = env::var_os("PAB_CONTROL_CA_CERT")
        .map(fs::read)
        .transpose()?;
    let connector = tls_connector(ca.as_deref())?;
    let result = loop {
        match register_open_endpoint(
            &control_url,
            &secret,
            OpenRegistrationKind::Device { name: name.clone() },
            connector.clone(),
            Duration::from_secs(10),
        )
        .await
        {
            Ok(result) => break result,
            Err(
                error @ (pab_agent_core::OpenRegistrationError::Timeout
                | pab_agent_core::OpenRegistrationError::WebSocket(_)
                | pab_agent_core::OpenRegistrationError::Server {
                    code: pab_protocol::ControlErrorCode::Internal,
                    ..
                }),
            ) => {
                tracing::warn!(%error, "device registration unavailable; retrying in 3s");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            Err(error) => return Err(error.into()),
        }
    };
    let EndpointRegistrationResult::Device {
        tenant_id,
        device_id,
        device_code,
        ..
    } = result
    else {
        return Err(BootstrapError::InvalidResult);
    };
    let config = ExecutorConfig::from_lookup(|key| match key {
        "PAB_TENANT_ID" => Some(OsString::from(tenant_id.to_string())),
        "PAB_DEVICE_ID" => Some(OsString::from(device_id.to_string())),
        "PAB_ENDPOINT_SECRET_FILE" => Some(secret_path.clone().into_os_string()),
        _ => env::var_os(key),
    })?;
    ensure_device_access(
        &config.task_database_file,
        tenant_id.to_string(),
        device_id.to_string(),
        device_code.to_string(),
    )
    .await?;
    for name in [
        "device-info.json",
        "current-password.txt",
        "device-credential.json",
    ] {
        let legacy = paths.root().join(name);
        if let Err(error) = fs::remove_file(&legacy)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %legacy.display(), %error, "could not remove old device access file");
        }
    }
    Ok(config)
}

async fn ensure_device_access(
    path: &Path,
    tenant_id: String,
    device_id: String,
    device_code: String,
) -> Result<(), BootstrapError> {
    let mut access = if path.exists() {
        match crate::device_access::load(path).await {
            Ok(access) => access,
            Err(crate::device_access::DeviceAccessError::Missing) => new_device_access(),
            Err(error) => return Err(error.into()),
        }
    } else {
        new_device_access()
    };
    access.tenant_id = tenant_id;
    access.device_id = device_id;
    access.device_code = device_code;
    let valid_password = if access.temporary_password.len() == 8 {
        match DeviceCredential::read(path).await {
            Ok(credential) => {
                credential
                    .verify(zeroize::Zeroizing::new(access.temporary_password.clone()))
                    .await?
            }
            Err(_) => false,
        }
    } else {
        false
    };
    if !valid_password {
        set_new_password(&mut access)?;
    }
    crate::device_access::save(path, &access).await?;
    Ok(())
}

pub async fn rotate_temporary_password(path: &Path) -> Result<(), BootstrapError> {
    let mut access = crate::device_access::load(path).await?;
    set_new_password(&mut access)?;
    crate::device_access::save(path, &access).await?;
    Ok(())
}

fn new_device_access() -> crate::device_access::DeviceAccess {
    crate::device_access::DeviceAccess {
        tenant_id: String::new(),
        device_id: String::new(),
        device_code: String::new(),
        temporary_password: String::new(),
        password_version: 0,
        password_hash: String::new(),
    }
}

fn set_new_password(access: &mut crate::device_access::DeviceAccess) -> Result<(), BootstrapError> {
    let password = generate_temporary_password();
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))
        .map_err(|_| BootstrapError::Hash)?
        .to_string();
    let version = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| BootstrapError::Clock)?
            .as_millis(),
    )
    .map_err(|_| BootstrapError::Clock)?;
    access.temporary_password = password;
    access.password_version = version;
    access.password_hash = hash;
    Ok(())
}

fn generate_temporary_password() -> String {
    // A 32-character alphabet keeps each character unbiased at five random bits.
    // Ambiguous characters (0/O and 1/I) are excluded for easier manual entry.
    const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut random = [0u8; 8];
    OsRng.fill_bytes(&mut random);
    random
        .iter()
        .map(|byte| char::from(ALPHABET[(byte & 31) as usize]))
        .collect()
}

pub async fn show_access() -> Result<(), BootstrapError> {
    let paths = DataPaths::for_scope(DataScope::Machine)?;
    let access = crate::device_access::load(&paths.executor_database()).await?;
    println!("Device code: {}", access.device_code);
    println!("Temporary password: {}", access.temporary_password);
    Ok(())
}

fn required(name: &'static str) -> Result<String, BootstrapError> {
    env::var(name).map_err(|_| BootstrapError::Missing(name))
}

#[derive(Debug, Error)]
pub enum BootstrapError {
    #[error("{0} must be set")]
    Missing(&'static str),
    #[error("device registration returned an unexpected result")]
    InvalidResult,
    #[error("system clock is invalid")]
    Clock,
    #[error("temporary password could not be hashed")]
    Hash,
    #[error(transparent)]
    Config(#[from] ExecutorConfigError),
    #[error(transparent)]
    Credential(#[from] crate::credential::DeviceCredentialError),
    #[error(transparent)]
    Access(#[from] crate::device_access::DeviceAccessError),
    #[error(transparent)]
    DataPath(#[from] pab_agent_core::DataPathError),
    #[error(transparent)]
    Secret(#[from] pab_agent_core::EndpointSecretError),
    #[error(transparent)]
    Registration(#[from] pab_agent_core::OpenRegistrationError),
    #[error(transparent)]
    Control(#[from] pab_agent_core::EndpointControlError),
    #[error(transparent)]
    Tls(#[from] pab_agent_core::TlsConnectorError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::{ensure_device_access, generate_temporary_password, rotate_temporary_password};
    use crate::credential::DeviceCredential;
    use crate::device_access;
    use zeroize::Zeroizing;

    #[test]
    fn temporary_password_is_eight_unambiguous_characters() {
        for _ in 0..100 {
            let password = generate_temporary_password();
            assert_eq!(password.len(), 8);
            assert!(
                password
                    .bytes()
                    .all(|byte| { b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789".contains(&byte) })
            );
        }
    }

    #[tokio::test]
    async fn rotated_password_matches_credential_and_invalidates_previous_password() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("executor.sqlite3");
        ensure_device_access(&path, "tenant".into(), "device".into(), "123456789".into())
            .await
            .unwrap();
        let previous_password = device_access::load(&path).await.unwrap().temporary_password;
        rotate_temporary_password(&path).await.unwrap();

        let current_password = device_access::load(&path).await.unwrap().temporary_password;
        let credential = DeviceCredential::read(&path).await.unwrap();
        assert_eq!(current_password.len(), 8);
        assert!(
            credential
                .verify(Zeroizing::new(current_password))
                .await
                .unwrap()
        );
        assert!(
            !credential
                .verify(Zeroizing::new(previous_password))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn existing_password_survives_restart_until_explicit_rotation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("executor.sqlite3");
        ensure_device_access(&path, "tenant".into(), "device".into(), "123456789".into())
            .await
            .unwrap();
        let first = device_access::load(&path).await.unwrap();

        ensure_device_access(&path, "tenant".into(), "device".into(), "123456789".into())
            .await
            .unwrap();
        let same = device_access::load(&path).await.unwrap();
        assert_eq!(same.temporary_password, first.temporary_password);
        assert_eq!(same.password_hash, first.password_hash);

        rotate_temporary_password(&path).await.unwrap();
        let rotated_password = device_access::load(&path).await.unwrap().temporary_password;
        assert_ne!(rotated_password, first.temporary_password);
        ensure_device_access(&path, "tenant".into(), "device".into(), "123456789".into())
            .await
            .unwrap();
        assert_eq!(
            device_access::load(&path).await.unwrap().temporary_password,
            rotated_password
        );
    }

    #[tokio::test]
    async fn initializes_access_in_an_existing_task_database() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("executor.sqlite3");
        let pool = sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        sqlx::query("CREATE TABLE task_records (id INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();

        ensure_device_access(&path, "tenant".into(), "device".into(), "123456789".into())
            .await
            .unwrap();
        assert_eq!(
            device_access::load(&path).await.unwrap().device_code,
            "123456789"
        );
        let existing: i64 = sqlx::query_scalar("SELECT count(*) FROM task_records")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(existing, 0);
    }
}
