use std::{fs, path::Path};

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

#[derive(Debug, Error)]
pub enum EndpointSecretError {
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
