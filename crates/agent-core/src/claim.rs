use std::time::Duration;

use pab_protocol::{
    ClaimId, ControlClientMessage, ControlServerMessage, DeviceCode, RequestId, TenantId,
};
use thiserror::Error;
use tokio_tungstenite::{
    Connector, connect_async_tls_with_config, tungstenite::client::IntoClientRequest,
};
use zeroize::{Zeroize, Zeroizing};

use crate::control::{receive, send};

pub async fn begin_device_claim(
    control_url: &str,
    username: String,
    password: Zeroizing<String>,
    device_code: DeviceCode,
    owner_tenant_id: TenantId,
    connector: Connector,
    timeout: Duration,
) -> Result<ClaimId, ClaimError> {
    let (claim_id, _) = begin_claim(
        control_url,
        username,
        password,
        device_code,
        Some(owner_tenant_id),
        connector,
        timeout,
    )
    .await?;
    Ok(claim_id)
}

pub async fn begin_personal_device_claim(
    control_url: &str,
    username: String,
    password: Zeroizing<String>,
    device_code: DeviceCode,
    connector: Connector,
    timeout: Duration,
) -> Result<(ClaimId, TenantId), ClaimError> {
    begin_claim(
        control_url,
        username,
        password,
        device_code,
        None,
        connector,
        timeout,
    )
    .await
}

async fn begin_claim(
    control_url: &str,
    username: String,
    password: Zeroizing<String>,
    device_code: DeviceCode,
    owner_tenant_id: Option<TenantId>,
    connector: Connector,
    timeout: Duration,
) -> Result<(ClaimId, TenantId), ClaimError> {
    if !control_url.starts_with("wss://") || timeout.is_zero() {
        return Err(ClaimError::InvalidConfig);
    }
    let request = control_url.into_client_request()?;
    let (mut socket, _) = tokio::time::timeout(
        timeout,
        connect_async_tls_with_config(request, None, false, Some(connector)),
    )
    .await
    .map_err(|_| ClaimError::Timeout)??;
    let login_id = RequestId::new();
    let mut login = ControlClientMessage::Login {
        request_id: login_id,
        username,
        password: password.to_string(),
    };
    send(&mut socket, &login, timeout).await?;
    if let ControlClientMessage::Login { password, .. } = &mut login {
        password.zeroize();
    }
    let personal_tenant_id = match receive(&mut socket, timeout).await? {
        ControlServerMessage::AccountAuthenticated {
            request_id,
            personal_tenant_id,
            ..
        } if request_id == login_id => personal_tenant_id,
        ControlServerMessage::Error {
            request_id: Some(request_id),
            code,
            message,
        } if request_id == login_id => return Err(ClaimError::Server { code, message }),
        _ => return Err(ClaimError::MismatchedResponse),
    };
    let owner_tenant_id = owner_tenant_id.unwrap_or(personal_tenant_id);
    let request_id = RequestId::new();
    send(
        &mut socket,
        &ControlClientMessage::BeginDeviceClaim {
            request_id,
            device_code,
            owner_tenant_id,
        },
        timeout,
    )
    .await?;
    match receive(&mut socket, timeout).await? {
        ControlServerMessage::DeviceClaimPending {
            request_id: result_id,
            claim_id,
        } if result_id == request_id => Ok((claim_id, owner_tenant_id)),
        ControlServerMessage::Error {
            request_id: Some(result_id),
            code,
            message,
        } if result_id == request_id => Err(ClaimError::Server { code, message }),
        _ => Err(ClaimError::MismatchedResponse),
    }
}

#[derive(Debug, Error)]
pub enum ClaimError {
    #[error("claim requires WSS and a nonzero timeout")]
    InvalidConfig,
    #[error("claim timed out")]
    Timeout,
    #[error("claim response did not match the request")]
    MismatchedResponse,
    #[error("server rejected claim with {code:?}: {message}")]
    Server {
        code: pab_protocol::ControlErrorCode,
        message: String,
    },
    #[error(transparent)]
    Control(#[from] crate::EndpointControlError),
    #[error(transparent)]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error(transparent)]
    Http(#[from] tokio_tungstenite::tungstenite::http::Error),
}
