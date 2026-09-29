use std::{fs, io::Write, path::Path};

use iroh_base::SecretKey;
use thiserror::Error;

const MAX_SECRET_TEXT_BYTES: usize = 256;

pub fn read_endpoint_secret(path: &Path) -> Result<SecretKey, EndpointSecretError> {
    let encoded = fs::read_to_string(path).map_err(|source| EndpointSecretError::Read {
        path: path.to_owned(),
        source,
    })?;
    let encoded = encoded.trim();
    if encoded.is_empty() {
        return Err(EndpointSecretError::Empty);
    }
    if encoded.len() > MAX_SECRET_TEXT_BYTES {
        return Err(EndpointSecretError::TooLarge);
    }
    encoded
        .parse()
        .map_err(|_| EndpointSecretError::InvalidEncoding)
}

pub fn load_or_create_endpoint_secret(path: &Path) -> Result<SecretKey, EndpointSecretError> {
    if path.exists() {
        restrict_private_file(path).map_err(|source| EndpointSecretError::Write {
            path: path.to_owned(),
            source,
        })?;
        return read_endpoint_secret(path);
    }
    crate::ensure_data_parent(path).map_err(|source| EndpointSecretError::Write {
        path: path.to_owned(),
        source,
    })?;
    let secret = SecretKey::generate();
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            for byte in secret.to_bytes() {
                write!(file, "{byte:02x}").map_err(|source| EndpointSecretError::Write {
                    path: path.to_owned(),
                    source,
                })?;
            }
            file.write_all(b"\n")
                .map_err(|source| EndpointSecretError::Write {
                    path: path.to_owned(),
                    source,
                })?;
            file.sync_all()
                .map_err(|source| EndpointSecretError::Write {
                    path: path.to_owned(),
                    source,
                })?;
            restrict_private_file(path).map_err(|source| EndpointSecretError::Write {
                path: path.to_owned(),
                source,
            })?;
            Ok(secret)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            read_endpoint_secret(path)
        }
        Err(source) => Err(EndpointSecretError::Write {
            path: path.to_owned(),
            source,
        }),
    }
}

pub fn restrict_private_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[derive(Debug, Error)]
pub enum EndpointSecretError {
    #[error("the endpoint secret file {path} could not be written: {source}")]
    Write {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("the endpoint secret file {path} could not be read: {source}")]
    Read {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("the endpoint secret file is empty")]
    Empty,
    #[error("the endpoint secret file is unexpectedly large")]
    TooLarge,
    #[error("the endpoint secret file does not contain a valid iroh secret key")]
    InvalidEncoding,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_iroh_hex_secret_without_exposing_it_in_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("endpoint.key");
        let expected = SecretKey::generate();
        std::fs::write(&path, format!("{}\n", hex_string(expected.to_bytes()))).unwrap();

        let actual = read_endpoint_secret(&path).unwrap();
        assert_eq!(actual.public(), expected.public());
    }

    fn hex_string(bytes: [u8; 32]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = String::with_capacity(64);
        for byte in bytes {
            encoded.push(HEX[usize::from(byte >> 4)] as char);
            encoded.push(HEX[usize::from(byte & 0x0f)] as char);
        }
        encoded
    }
}
