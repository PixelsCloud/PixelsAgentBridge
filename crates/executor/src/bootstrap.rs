use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
use pab_agent_core::{
    AuthenticatedControlConnection, DataPaths, DataScope, EndpointControlConfig,
    OpenRegistrationKind, load_or_create_endpoint_secret, register_open_endpoint, tls_connector,
};
use pab_protocol::{
    ClaimId, DeploymentId, DeviceId, EndpointProofPrincipal, EndpointRegistrationResult, TenantId,
};
use rand_core::{OsRng, RngCore};
use thiserror::Error;

use crate::{ExecutorConfig, ExecutorConfigError};

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
    let deployment_id = required("PAB_DEPLOYMENT_ID")?
        .parse()
        .map_err(|_| BootstrapError::InvalidDeployment)?;
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
            deployment_id,
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
    rotate_temporary_password(
        &config.device_credential_file,
        paths.root().join("current-password.txt"),
    )?;
    let device_info = serde_json::json!({
        "deployment_id": deployment_id,
        "tenant_id": tenant_id,
        "device_id": device_id,
        "device_code": device_code,
    });
    let device_info_bytes = serde_json::to_vec_pretty(&device_info)?;
    atomic_private_write(&paths.root().join("device-info.json"), &device_info_bytes)?;
    Ok(config)
}

fn rotate_temporary_password(
    credential_path: &Path,
    password_path: PathBuf,
) -> Result<(), BootstrapError> {
    let mut random = [0u8; 16];
    OsRng.fill_bytes(&mut random);
    let password = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
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
    atomic_private_write(
        credential_path,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "password_version": version,
            "password_hash": hash,
        }))?
        .as_slice(),
    )?;
    atomic_private_write(&password_path, format!("{password}\n").as_bytes())?;
    Ok(())
}

fn atomic_private_write(path: &Path, bytes: &[u8]) -> Result<(), BootstrapError> {
    pab_agent_core::ensure_data_parent(path)?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    use std::io::Write;
    file.write_all(bytes)?;
    file.sync_all()?;
    pab_agent_core::restrict_private_file(&temp)?;
    fs::rename(&temp, path)?;
    Ok(())
}

pub fn show_access() -> Result<(), BootstrapError> {
    let paths = DataPaths::for_scope(DataScope::Machine)?;
    let info = fs::read_to_string(paths.root().join("device-info.json"))?;
    let password = fs::read_to_string(paths.root().join("current-password.txt"))?;
    let info: serde_json::Value = serde_json::from_str(&info)?;
    println!("Device code: {}", info["device_code"]);
    println!("Temporary password: {}", password.trim());
    Ok(())
}

pub async fn approve_claim(claim_id: ClaimId) -> Result<(), BootstrapError> {
    let paths = DataPaths::for_scope(DataScope::Machine)?;
    let info: serde_json::Value =
        serde_json::from_slice(&fs::read(paths.root().join("device-info.json"))?)?;
    let parse = |key: &'static str| -> Result<String, BootstrapError> {
        Ok(info[key]
            .as_str()
            .ok_or(BootstrapError::InvalidResult)?
            .to_owned())
    };
    let deployment_id: DeploymentId = parse("deployment_id")?
        .parse()
        .map_err(|_| BootstrapError::InvalidResult)?;
    let tenant_id: TenantId = parse("tenant_id")?
        .parse()
        .map_err(|_| BootstrapError::InvalidResult)?;
    let device_id: DeviceId = parse("device_id")?
        .parse()
        .map_err(|_| BootstrapError::InvalidResult)?;
    let secret_path = env::var_os("PAB_ENDPOINT_SECRET_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.executor_endpoint_secret());
    let secret = pab_agent_core::read_endpoint_secret(&secret_path)?;
    let ca = env::var_os("PAB_CONTROL_CA_CERT")
        .map(fs::read)
        .transpose()?;
    let config = EndpointControlConfig {
        url: required("PAB_CONTROL_URL")?,
        deployment_id,
        tenant_id,
        principal: EndpointProofPrincipal::Device { device_id },
        operation_timeout: Duration::from_secs(10),
    };
    let mut connection =
        AuthenticatedControlConnection::connect(&config, &secret, tls_connector(ca.as_deref())?)
            .await?;
    let (claimed_device, owner_tenant_id) = connection
        .approve_device_claim(claim_id, Duration::from_secs(10))
        .await?;
    if claimed_device != device_id {
        return Err(BootstrapError::InvalidResult);
    }
    println!("Approved claim {claim_id} for device {device_id} into {owner_tenant_id}");
    Ok(())
}

fn required(name: &'static str) -> Result<String, BootstrapError> {
    env::var(name).map_err(|_| BootstrapError::Missing(name))
}

#[derive(Debug, Error)]
pub enum BootstrapError {
    #[error("{0} must be set")]
    Missing(&'static str),
    #[error("PAB_DEPLOYMENT_ID is invalid")]
    InvalidDeployment,
    #[error("device registration returned an unexpected result")]
    InvalidResult,
    #[error("system clock is invalid")]
    Clock,
    #[error("temporary password could not be hashed")]
    Hash,
    #[error(transparent)]
    Config(#[from] ExecutorConfigError),
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
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
