use std::{env, fs, io::Write, path::PathBuf, process::ExitCode, time::Duration};

use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
use pab_agent_core::{DataPaths, DataScope, enroll_account_with_device, tls_connector};
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
    let database = output_directory.join("executor.sqlite3");
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&database)
            .create_if_missing(true),
    )
    .await?;
    pab_agent_core::restrict_private_file(&database)?;
    sqlx::query(
        "CREATE TABLE device_access (id INTEGER PRIMARY KEY CHECK (id = 1), \
         tenant_id TEXT NOT NULL, device_id TEXT NOT NULL, \
         device_code TEXT NOT NULL, temporary_password TEXT NOT NULL, \
         password_version INTEGER NOT NULL, password_hash TEXT NOT NULL)",
    )
    .execute(&pool)
    .await?;
    sqlx::query("INSERT INTO device_access VALUES (1, ?, ?, ?, ?, 1, ?)")
        .bind(enrollment.tenant_id.to_string())
        .bind(enrollment.device_id.to_string())
        .bind(enrollment.device_code.to_string())
        .bind(device_password.as_str())
        .bind(password_hash)
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE enrollment (id INTEGER PRIMARY KEY CHECK (id = 1), \
         user_id TEXT NOT NULL, control_url TEXT NOT NULL, relay_urls TEXT NOT NULL)",
    )
    .execute(&pool)
    .await?;
    sqlx::query("INSERT INTO enrollment VALUES (1, ?, ?, ?)")
        .bind(enrollment.user_id.to_string())
        .bind(control_url)
        .bind(relay_urls)
        .execute(&pool)
        .await?;
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
        "executor.sqlite3",
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
    Sql(#[from] sqlx::Error),
}

impl From<argon2::password_hash::Error> for EnrollCliError {
    fn from(value: argon2::password_hash::Error) -> Self {
        Self::PasswordHash(value.to_string())
    }
}
