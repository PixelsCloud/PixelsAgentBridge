use std::fs;

use iroh_base::SecretKey;
use pab_transport::{PabEndpoint, PabEndpointConfig, PabEndpointError};
use thiserror::Error;

use crate::ExecutorConfig;

pub(crate) async fn bind_endpoint(
    config: &ExecutorConfig,
    secret: SecretKey,
) -> Result<PabEndpoint, ExecutorNetworkError> {
    let mut endpoint_config = PabEndpointConfig::new(config.relay_urls.clone())?;
    if let Some(path) = &config.relay_ca_cert {
        let pem = fs::read(path).map_err(|source| ExecutorNetworkError::RelayCaFile {
            path: path.clone(),
            source,
        })?;
        endpoint_config = endpoint_config.with_extra_ca_pem(&pem)?;
    }
    Ok(PabEndpoint::bind(endpoint_config, secret).await?)
}

#[derive(Debug, Error)]
pub enum ExecutorNetworkError {
    #[error("the Relay CA file {path} could not be read: {source}")]
    RelayCaFile {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Endpoint(#[from] PabEndpointError),
}
