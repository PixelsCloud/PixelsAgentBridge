use std::{fs, time::SystemTime};

use iroh_base::SecretKey;
use pab_protocol::{
    DEVICE_NETWORK_SCHEMA_VERSION, DeviceNetworkUpdate, DeviceRef, EndpointInstanceId, EndpointKey,
};
use pab_transport::{PabEndpoint, PabEndpointAddress, PabEndpointConfig, PabEndpointError};
use thiserror::Error;
use tokio::sync::watch;

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

pub(crate) fn watch_device_network(
    endpoint: &PabEndpoint,
    device_ref: DeviceRef,
    endpoint_key: EndpointKey,
) -> Result<watch::Receiver<DeviceNetworkUpdate>, ExecutorNetworkError> {
    let mut addresses = endpoint.watch_address();
    let endpoint_instance_id = EndpointInstanceId::new();
    let initial = network_update(
        device_ref,
        endpoint_key,
        endpoint_instance_id,
        1,
        addresses.borrow().clone(),
    )?;
    let (sender, receiver) = watch::channel(initial);
    tokio::spawn(async move {
        let mut revision = 1_u64;
        while addresses.changed().await.is_ok() {
            revision = revision.saturating_add(1);
            let address = addresses.borrow_and_update().clone();
            let Ok(update) = network_update(
                device_ref,
                endpoint_key,
                endpoint_instance_id,
                revision,
                address,
            ) else {
                break;
            };
            if sender.send(update).is_err() {
                break;
            }
        }
    });
    Ok(receiver)
}

fn network_update(
    device_ref: DeviceRef,
    endpoint_key: EndpointKey,
    endpoint_instance_id: EndpointInstanceId,
    address_revision: u64,
    address: PabEndpointAddress,
) -> Result<DeviceNetworkUpdate, ExecutorNetworkError> {
    Ok(DeviceNetworkUpdate {
        schema_version: DEVICE_NETWORK_SCHEMA_VERSION,
        device_ref,
        endpoint_key,
        endpoint_instance_id,
        address_revision,
        relay_urls: address.relay_urls,
        direct_addresses: address.direct_addresses,
        observed_at_unix_ms: unix_millis(SystemTime::now())?,
    })
}

fn unix_millis(now: SystemTime) -> Result<i64, ExecutorNetworkError> {
    let value = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| ExecutorNetworkError::InvalidSystemTime)?
        .as_millis();
    i64::try_from(value).map_err(|_| ExecutorNetworkError::InvalidSystemTime)
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
    #[error("system time is outside the supported Unix timestamp range")]
    InvalidSystemTime,
}
