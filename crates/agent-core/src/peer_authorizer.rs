use std::time::Duration;

use pab_protocol::{AuthorizedDevicePeer, EndpointKey};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use crate::EndpointControlError;

pub(crate) struct PeerAuthorizationRequest {
    pub peer_endpoint_key: EndpointKey,
    pub response: oneshot::Sender<Result<AuthorizedDevicePeer, EndpointControlError>>,
}

#[derive(Clone)]
pub struct DevicePeerAuthorizer {
    sender: mpsc::Sender<PeerAuthorizationRequest>,
    timeout: Duration,
}

impl DevicePeerAuthorizer {
    pub(crate) fn new(sender: mpsc::Sender<PeerAuthorizationRequest>, timeout: Duration) -> Self {
        Self { sender, timeout }
    }

    pub async fn authorize(
        &self,
        peer_endpoint_key: EndpointKey,
    ) -> Result<AuthorizedDevicePeer, PeerAuthorizationError> {
        let (response, receiver) = oneshot::channel();
        tokio::time::timeout(
            self.timeout,
            self.sender.send(PeerAuthorizationRequest {
                peer_endpoint_key,
                response,
            }),
        )
        .await
        .map_err(|_| PeerAuthorizationError::Timeout)?
        .map_err(|_| PeerAuthorizationError::Unavailable)?;
        tokio::time::timeout(self.timeout, receiver)
            .await
            .map_err(|_| PeerAuthorizationError::Timeout)?
            .map_err(|_| PeerAuthorizationError::Unavailable)?
            .map_err(PeerAuthorizationError::Control)
    }
}

#[derive(Debug, Error)]
pub enum PeerAuthorizationError {
    #[error("the endpoint control supervisor is unavailable")]
    Unavailable,
    #[error("device peer authorization timed out")]
    Timeout,
    #[error(transparent)]
    Control(EndpointControlError),
}
