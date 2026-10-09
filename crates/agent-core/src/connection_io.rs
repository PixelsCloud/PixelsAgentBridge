use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use pab_protocol::{
    ControlClientMessage, ControlServerMessage, DeviceNetworkUpdate, DeviceRef, EndpointKey,
    RequestId,
};
use tokio_tungstenite::tungstenite::Message;

use crate::{AuthenticatedControlConnection, EndpointControlError, control::send};

pub(crate) enum IncomingControlFrame {
    Server(ControlServerMessage),
    Pong(Vec<u8>),
}

impl AuthenticatedControlConnection {
    pub(crate) async fn send_ping(
        &mut self,
        payload: Vec<u8>,
        timeout: Duration,
    ) -> Result<(), EndpointControlError> {
        if timeout.is_zero() {
            return Err(EndpointControlError::InvalidTimeout);
        }
        tokio::time::timeout(timeout, self.socket.send(Message::Ping(payload.into())))
            .await
            .map_err(|_| EndpointControlError::Timeout)??;
        Ok(())
    }

    pub(crate) async fn send_device_network(
        &mut self,
        update: &DeviceNetworkUpdate,
        timeout: Duration,
    ) -> Result<RequestId, EndpointControlError> {
        let request_id = RequestId::new();
        send(
            &mut self.socket,
            &ControlClientMessage::PublishDeviceNetwork {
                request_id,
                update: Box::new(update.clone()),
            },
            timeout,
        )
        .await?;
        Ok(request_id)
    }

    pub(crate) async fn send_authorize_device_peer(
        &mut self,
        peer_endpoint_key: EndpointKey,
        authenticated: bool,
        timeout: Duration,
    ) -> Result<RequestId, EndpointControlError> {
        let request_id = RequestId::new();
        send(
            &mut self.socket,
            &if authenticated {
                ControlClientMessage::ReportAuthenticatedDevicePeer {
                    request_id,
                    peer_endpoint_key,
                }
            } else {
                ControlClientMessage::AuthorizeDevicePeer {
                    request_id,
                    peer_endpoint_key,
                }
            },
            timeout,
        )
        .await?;
        Ok(request_id)
    }

    pub(crate) async fn send_get_device_network(
        &mut self,
        device_ref: DeviceRef,
        timeout: Duration,
    ) -> Result<RequestId, EndpointControlError> {
        let request_id = RequestId::new();
        send(
            &mut self.socket,
            &ControlClientMessage::GetDeviceNetwork {
                request_id,
                device_ref,
            },
            timeout,
        )
        .await?;
        Ok(request_id)
    }

    pub(crate) async fn send_resolve_device_code(
        &mut self,
        device_code: pab_protocol::DeviceCode,
        timeout: Duration,
    ) -> Result<RequestId, EndpointControlError> {
        let request_id = RequestId::new();
        send(
            &mut self.socket,
            &ControlClientMessage::ResolveDeviceCode {
                request_id,
                device_code,
            },
            timeout,
        )
        .await?;
        Ok(request_id)
    }

    pub(crate) async fn send_get_device_presence(
        &mut self,
        device_code: pab_protocol::DeviceCode,
        timeout: Duration,
    ) -> Result<RequestId, EndpointControlError> {
        let request_id = RequestId::new();
        send(
            &mut self.socket,
            &ControlClientMessage::GetDevicePresence {
                request_id,
                device_code,
            },
            timeout,
        )
        .await?;
        Ok(request_id)
    }

    pub(crate) async fn send_list_devices(
        &mut self,
        timeout: Duration,
    ) -> Result<RequestId, EndpointControlError> {
        let request_id = RequestId::new();
        send(
            &mut self.socket,
            &ControlClientMessage::ListDevices { request_id },
            timeout,
        )
        .await?;
        Ok(request_id)
    }

    pub(crate) async fn next_frame(
        &mut self,
    ) -> Result<IncomingControlFrame, EndpointControlError> {
        loop {
            let message = self
                .socket
                .next()
                .await
                .ok_or(EndpointControlError::Closed)??;
            match message {
                Message::Text(text) => {
                    return Ok(IncomingControlFrame::Server(serde_json::from_str(
                        text.as_str(),
                    )?));
                }
                Message::Pong(payload) => {
                    return Ok(IncomingControlFrame::Pong(payload.to_vec()));
                }
                Message::Ping(payload) => self.socket.send(Message::Pong(payload)).await?,
                Message::Close(_) => return Err(EndpointControlError::Closed),
                Message::Binary(_) | Message::Frame(_) => {
                    return Err(EndpointControlError::UnexpectedMessage);
                }
            }
        }
    }

    pub async fn heartbeat(
        &mut self,
        payload: Vec<u8>,
        timeout: Duration,
    ) -> Result<(), EndpointControlError> {
        self.send_ping(payload.clone(), timeout).await?;
        tokio::time::timeout(timeout, async {
            loop {
                match self.next_frame().await? {
                    IncomingControlFrame::Pong(received) if received == payload => return Ok(()),
                    IncomingControlFrame::Pong(_) => {}
                    IncomingControlFrame::Server(_) => {
                        return Err(EndpointControlError::UnexpectedMessage);
                    }
                }
            }
        })
        .await
        .map_err(|_| EndpointControlError::Timeout)?
    }

    pub async fn close(mut self) -> Result<(), EndpointControlError> {
        self.socket.close(None).await?;
        Ok(())
    }
}
