use std::{fs::File, io::BufReader, path::Path, sync::Arc};

use rustls::{ClientConfig, RootCertStore, ServerConfig};
use thiserror::Error;
use tokio_tungstenite::Connector;

pub fn load_server_config(
    certificate_path: &Path,
    private_key_path: &Path,
) -> Result<ServerConfig, RelayTlsError> {
    let mut certificate_reader = BufReader::new(File::open(certificate_path)?);
    let certificates =
        rustls_pemfile::certs(&mut certificate_reader).collect::<Result<Vec<_>, _>>()?;
    let mut key_reader = BufReader::new(File::open(private_key_path)?);
    let private_key =
        rustls_pemfile::private_key(&mut key_reader)?.ok_or(RelayTlsError::PrivateKeyMissing)?;
    Ok(
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(certificates, private_key)?,
    )
}

pub fn control_connector(ca_certificate_path: Option<&Path>) -> Result<Connector, RelayTlsError> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(path) = ca_certificate_path {
        let mut reader = BufReader::new(File::open(path)?);
        for certificate in rustls_pemfile::certs(&mut reader) {
            roots.add(certificate?)?;
        }
    }
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(config)))
}

#[derive(Debug, Error)]
pub enum RelayTlsError {
    #[error("Relay TLS file could not be read: {0}")]
    Io(#[from] std::io::Error),
    #[error("Relay TLS certificate or key is invalid: {0}")]
    Certificate(#[from] rustls::Error),
    #[error("Relay TLS private key was not found")]
    PrivateKeyMissing,
}
