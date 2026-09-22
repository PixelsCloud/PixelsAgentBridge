use std::io::Cursor;

use rustls::{ClientConfig, RootCertStore};
use thiserror::Error;
use tokio_tungstenite::Connector;

pub fn tls_connector(extra_ca_pem: Option<&[u8]>) -> Result<Connector, TlsConnectorError> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(pem) = extra_ca_pem {
        let certificates =
            rustls_pemfile::certs(&mut Cursor::new(pem)).collect::<Result<Vec<_>, _>>()?;
        if certificates.is_empty() {
            return Err(TlsConnectorError::NoCertificates);
        }
        for certificate in certificates {
            roots.add(certificate)?;
        }
    }
    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Connector::Rustls(config.into()))
}

#[derive(Debug, Error)]
pub enum TlsConnectorError {
    #[error("the extra CA file contains no certificates")]
    NoCertificates,
    #[error("the extra CA file is not valid PEM")]
    Pem(#[from] std::io::Error),
    #[error("the extra CA certificate is not valid")]
    Certificate(#[from] rustls::Error),
}
