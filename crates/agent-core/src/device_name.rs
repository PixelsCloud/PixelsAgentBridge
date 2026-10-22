//! A short authenticated API connection updates metadata without touching the
//! existing supervisor or its established execution sessions.
use crate::{
    AuthenticatedControlConnection, EndpointControlError,
    control::{receive, send},
};
use pab_protocol::{ControlClientMessage, ControlServerMessage, RequestId};
use std::time::Duration;

impl AuthenticatedControlConnection {
    pub async fn rename_own_device(
        &mut self,
        name: &str,
        timeout: Duration,
    ) -> Result<(), EndpointControlError> {
        if timeout.is_zero() {
            return Err(EndpointControlError::InvalidTimeout);
        }
        let request_id = RequestId::new();
        send(
            &mut self.socket,
            &ControlClientMessage::RenameOwnDevice {
                request_id,
                name: name.into(),
            },
            timeout,
        )
        .await?;
        match receive(&mut self.socket, timeout).await? {
            ControlServerMessage::OwnDeviceRenamed {
                request_id: response_id,
                name: returned,
            } if request_id == response_id && returned == name => Ok(()),
            ControlServerMessage::Error {
                request_id: Some(response_id),
                code,
                message,
            } if request_id == response_id => Err(EndpointControlError::Server { code, message }),
            _ => Err(EndpointControlError::MismatchedResponse),
        }
    }
}
