use std::{env, fs, io::Write, path::PathBuf, process::ExitCode, time::Duration};

use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
use pab_agent_core::{DataPaths, DataScope, enroll_account_with_device, tls_connector};
use pab_protocol::DeploymentId;
use rand_core::OsRng;
use thiserror::Error;
use zeroize::Zeroizing;

#[tokio::main]
async fn main() -> ExitCode {
    let log_root = match DataPaths::for_scope(DataScope::User) {
        Ok(paths) => paths.root().to_path_buf(),
        Err(error) => {
            eprintln!("pab-enroll: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = pab_logging::init("enroll", &log_root) {
        eprintln!("pab-enroll: {error}");
        return ExitCode::FAILURE;
    }
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "enrollment failed");
            eprintln!("pab-enroll: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), EnrollCliError> {
    let mut args = env::args().skip(1);
    let username = args.next().ok_or(EnrollCliError::Usage)?;
    let account_password_file = PathBuf::from(args.next().ok_or(EnrollCliError::Usage)?);
    let device_name = args.next().ok_or(EnrollCliError::Usage)?;
    let device_password_file = PathBuf::from(args.next().ok_or(EnrollCliError::Usage)?);
    let output_directory = match args.next() {
        Some(path) => PathBuf::from(path),
        None => DataPaths::for_scope(DataScope::User)?.root().to_path_buf(),
    };
    if args.next().is_some() {
        return Err(EnrollCliError::Usage);
    }
    let control_url =
        env::var("PAB_CONTROL_URL").map_err(|_| EnrollCliError::MissingEnv("PAB_CONTROL_URL"))?;
    let deployment_id = env::var("PAB_DEPLOYMENT_ID")
        .map_err(|_| EnrollCliError::MissingEnv("PAB_DEPLOYMENT_ID"))?
        .parse::<DeploymentId>()
        .map_err(|error| EnrollCliError::Deployment(error.to_string()))?;
    let relay_urls =
        env::var("PAB_RELAY_URLS").map_err(|_| EnrollCliError::MissingEnv("PAB_RELAY_URLS"))?;
    let extra_ca = env::var_os("PAB_CONTROL_CA_CERT")
        .map(fs::read)
        .transpose()?;
    let connector = tls_connector(extra_ca.as_deref())?;
    let account_password = read_password(&account_password_file)?;
    let device_password = read_password(&device_password_file)?;
    prepare_output_directory(&output_directory)?;
    let password_hash = Argon2::default()
        .hash_password(
            device_password.as_bytes(),
            &SaltString::generate(&mut OsRng),
        )?
        .to_string();
    let enrollment = enroll_account_with_device(
        &control_url,
        deployment_id,
        username,
        account_password,
        device_name,
        connector,
        Duration::from_secs(15),
    )
    .await?;

    write_new(
        output_directory.join("bridge-endpoint.key"),
        format!(
            "{}\n",
            secret_hex(enrollment.user_endpoint_secret.to_bytes())
        )
        .as_bytes(),
    )?;
    write_new(
        output_directory.join("device-endpoint.key"),
        format!(
            "{}\n",
            secret_hex(enrollment.device_endpoint_secret.to_bytes())
        )
        .as_bytes(),
    )?;
    let credential = serde_json::to_vec_pretty(&serde_json::json!({
        "schema_version": 1,
        "password_version": 1,
        "password_hash": password_hash,
    }))?;
    write_new(output_directory.join("device-credential.json"), &credential)?;
    let manifest = serde_json::to_vec_pretty(&serde_json::json!({
        "deployment_id": deployment_id,
        "tenant_id": enrollment.tenant_id,
        "user_id": enrollment.user_id,
        "device_id": enrollment.device_id,
        "device_code": enrollment.device_code,
        "control_url": control_url,
        "relay_urls": relay_urls,
    }))?;
    write_new(output_directory.join("enrollment.json"), &manifest)?;
    println!(
        "enrolled tenant={} user={} device={} code={}",
        enrollment.tenant_id, enrollment.user_id, enrollment.device_id, enrollment.device_code
    );
    Ok(())
}

fn prepare_output_directory(path: &PathBuf) -> Result<(), EnrollCliError> {
    pab_agent_core::ensure_data_dir(path)?;
    for name in [
        "bridge-endpoint.key",
        "device-endpoint.key",
        "device-credential.json",
        "enrollment.json",
    ] {
        let output = path.join(name);
        if output.exists() {
            return Err(EnrollCliError::OutputExists(output));
        }
    }
    Ok(())
}

fn read_password(path: &PathBuf) -> Result<Zeroizing<String>, EnrollCliError> {
    let bytes = fs::read(path)?;
    let mut value = String::from_utf8(bytes).map_err(|_| EnrollCliError::PasswordEncoding)?;
    value.truncate(value.trim_end_matches(['\r', '\n']).len());
    if value.is_empty() {
        return Err(EnrollCliError::EmptyPassword);
    }
    Ok(Zeroizing::new(value))
}

fn write_new(path: PathBuf, bytes: &[u8]) -> Result<(), EnrollCliError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|source| EnrollCliError::Write {
            path: path.clone(),
            source,
        })?;
    file.write_all(bytes)
        .map_err(|source| EnrollCliError::Write { path, source })
}

fn secret_hex(bytes: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

#[derive(Debug, Error)]
enum EnrollCliError {
    #[error(transparent)]
    DataPath(#[from] pab_agent_core::DataPathError),
    #[error(
        "usage: pab-enroll <username> <account-password-file> <device-name> <device-password-file> [output-directory]"
    )]
    Usage,
    #[error("{0} must be set")]
    MissingEnv(&'static str),
    #[error("PAB_DEPLOYMENT_ID is invalid: {0}")]
    Deployment(String),
    #[error("password file must contain UTF-8 text")]
    PasswordEncoding,
    #[error("password file is empty")]
    EmptyPassword,
    #[error("enrollment output already exists: {0}")]
    OutputExists(PathBuf),
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Tls(#[from] pab_agent_core::TlsConnectorError),
    #[error(transparent)]
    Enrollment(#[from] pab_agent_core::EnrollmentError),
    #[error("device password hashing failed: {0}")]
    PasswordHash(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl From<argon2::password_hash::Error> for EnrollCliError {
    fn from(value: argon2::password_hash::Error) -> Self {
        Self::PasswordHash(value.to_string())
    }
}
