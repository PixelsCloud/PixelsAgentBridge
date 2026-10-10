#![forbid(unsafe_code)]

mod rotating_file;

use std::{env, path::Path, sync::Arc};

use thiserror::Error;
use tracing_subscriber::{EnvFilter, fmt};

use rotating_file::{LogWriter, RotatingFile};

pub const MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_LOG_FILES: usize = 5;

pub fn init(role: &str, default_root: &Path) -> Result<(), LoggingError> {
    let root = env::var_os("PAB_LOG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| default_root.join("logs"));
    let level = env::var("PAB_LOG_LEVEL").unwrap_or_else(|_| "info".into());
    let level = if EnvFilter::try_new(&level).is_ok() {
        level
    } else {
        "info".into()
    };
    init_configured(role, &root, &level)
}

/// Explicit service settings; client init() retains its existing behavior.
pub fn init_configured(role: &str, root: &Path, level: &str) -> Result<(), LoggingError> {
    let filter = EnvFilter::try_new(level)
        .map_err(|_| LoggingError::Subscriber("invalid log.level".into()))?;
    let writer = Arc::new(RotatingFile::open(
        root,
        role,
        MAX_LOG_BYTES,
        MAX_LOG_FILES,
    )?);
    fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(move || LogWriter::new(Arc::clone(&writer)))
        .try_init()
        .map_err(|error| LoggingError::Subscriber(error.to_string()))?;
    let location = root.join(format!("{role}.log"));
    tracing::info!(path = %location.display(), "file logging ready");
    std::panic::set_hook(Box::new(|panic| {
        if let Some(location) = panic.location() {
            tracing::error!(
                file = location.file(),
                line = location.line(),
                "process panicked"
            );
        } else {
            tracing::error!("process panicked");
        }
    }));
    Ok(())
}

#[derive(Debug, Error)]
pub enum LoggingError {
    #[error("could not initialize log file: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not install logging subscriber: {0}")]
    Subscriber(String),
}
