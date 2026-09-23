use std::time::Duration;

use pab_protocol::{DeviceCode, DeviceDirectoryEntry, DeviceNetworkSnapshot, DeviceRef};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use crate::EndpointControlError;

pub(crate) struct DeviceNetworkRequest {
    pub lookup: DeviceNetworkLookup,
    pub response: oneshot::Sender<Result<ResolvedDevice, EndpointControlError>>,
}

pub(crate) enum ResolvedDevice {
    Network(DeviceNetworkSnapshot),
    Ref(DeviceRef),
    List(Vec<DeviceDirectoryEntry>),
}

#[derive(Clone, Copy)]
pub(crate) enum DeviceNetworkLookup {
    Ref(DeviceRef),
    Code(DeviceCode),
    List,
}

#[derive(Clone)]
pub struct DeviceNetworkResolver {
    sender: mpsc::Sender<DeviceNetworkRequest>,
    timeout: Duration,
}

impl DeviceNetworkResolver {
    pub(crate) fn new(sender: mpsc::Sender<DeviceNetworkRequest>, timeout: Duration) -> Self {
        Self { sender, timeout }
    }

    pub async fn resolve(
        &self,
        device_ref: DeviceRef,
    ) -> Result<DeviceNetworkSnapshot, DeviceNetworkResolutionError> {
        match self
            .resolve_lookup(DeviceNetworkLookup::Ref(device_ref))
            .await?
        {
            ResolvedDevice::Network(snapshot) => Ok(snapshot),
            ResolvedDevice::Ref(_) | ResolvedDevice::List(_) => {
                Err(DeviceNetworkResolutionError::UnexpectedResponse)
            }
        }
    }

    pub async fn resolve_code(
        &self,
        device_code: DeviceCode,
    ) -> Result<DeviceRef, DeviceNetworkResolutionError> {
        match self
            .resolve_lookup(DeviceNetworkLookup::Code(device_code))
            .await?
        {
            ResolvedDevice::Ref(device_ref) => Ok(device_ref),
            ResolvedDevice::Network(_) | ResolvedDevice::List(_) => {
                Err(DeviceNetworkResolutionError::UnexpectedResponse)
            }
        }
    }

    pub async fn list_devices(
        &self,
    ) -> Result<Vec<DeviceDirectoryEntry>, DeviceNetworkResolutionError> {
        match self.resolve_lookup(DeviceNetworkLookup::List).await? {
            ResolvedDevice::List(devices) => Ok(devices),
            _ => Err(DeviceNetworkResolutionError::UnexpectedResponse),
        }
    }

    async fn resolve_lookup(
        &self,
        lookup: DeviceNetworkLookup,
    ) -> Result<ResolvedDevice, DeviceNetworkResolutionError> {
        let (response, receiver) = oneshot::channel();
        tokio::time::timeout(
            self.timeout,
            self.sender.send(DeviceNetworkRequest { lookup, response }),
        )
        .await
        .map_err(|_| DeviceNetworkResolutionError::Timeout)?
        .map_err(|_| DeviceNetworkResolutionError::Unavailable)?;
        tokio::time::timeout(self.timeout, receiver)
            .await
            .map_err(|_| DeviceNetworkResolutionError::Timeout)?
            .map_err(|_| DeviceNetworkResolutionError::Unavailable)?
            .map_err(DeviceNetworkResolutionError::Control)
    }
}

#[derive(Debug, Error)]
pub enum DeviceNetworkResolutionError {
    #[error("the endpoint control supervisor is unavailable")]
    Unavailable,
    #[error("device network resolution timed out")]
    Timeout,
    #[error("the control server returned an unexpected resolution response")]
    UnexpectedResponse,
    #[error(transparent)]
    Control(EndpointControlError),
}
