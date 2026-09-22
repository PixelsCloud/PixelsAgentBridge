use std::time::Duration;

use pab_protocol::{DeviceNetworkSnapshot, DeviceRef};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use crate::EndpointControlError;

pub(crate) struct DeviceNetworkRequest {
    pub device_ref: DeviceRef,
    pub response: oneshot::Sender<Result<DeviceNetworkSnapshot, EndpointControlError>>,
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
        let (response, receiver) = oneshot::channel();
        tokio::time::timeout(
            self.timeout,
            self.sender.send(DeviceNetworkRequest {
                device_ref,
                response,
            }),
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
    #[error(transparent)]
    Control(EndpointControlError),
}
