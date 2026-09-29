use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use pab_protocol::{
    ControlClientMessage, ControlErrorCode, ControlServerMessage, DeploymentId, EndpointKey,
    EndpointProofPrincipal, EndpointRegistration, EndpointRegistrationResult, RequestId, TenantId,
    TrafficScopeOptions, UserId,
};
use thiserror::Error;
use tokio_tungstenite::{
    Connector, connect_async_tls_with_config, tungstenite::client::IntoClientRequest,
};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    AuthenticatedControlConnection, EndpointControlConfig, EndpointControlError,
    EndpointSecretError, EnrollmentError,
    control::{receive, send},
    enrollment::register_endpoint,
    load_or_create_endpoint_secret,
};

pub struct AccountScopeRegistration {
    pub user_id: UserId,
    pub tenant_id: TenantId,
    pub team_name: Option<String>,
    pub endpoint_secret_file: PathBuf,
}

pub async fn login_traffic_scopes(
    control_url: &str,
    username: String,
    password: Zeroizing<String>,
    connector: Connector,
    timeout: Duration,
) -> Result<(UserId, TrafficScopeOptions), AccountScopeError> {
    let (identity, _) = login(control_url, username, password, connector, timeout).await?;
    Ok(identity)
}

pub async fn register_account_traffic_scope(
    control_url: &str,
    deployment_id: DeploymentId,
    username: String,
    password: Zeroizing<String>,
    selected_tenant_id: TenantId,
    endpoint_secret_dir: &Path,
    connector: Connector,
    timeout: Duration,
) -> Result<AccountScopeRegistration, AccountScopeError> {
    let ((user_id, options), mut socket) =
        login(control_url, username, password, connector.clone(), timeout).await?;
    if selected_tenant_id != options.personal_tenant_id
        && !options
            .teams
            .iter()
            .any(|team| team.tenant_id == selected_tenant_id)
    {
        return Err(AccountScopeError::ScopeUnavailable);
    }
    let team_name = options
        .teams
        .iter()
        .find(|team| team.tenant_id == selected_tenant_id)
        .map(|team| team.name.clone());

    let endpoint_secret_file = endpoint_secret_dir.join(format!(
        "account-{user_id}-{selected_tenant_id}-endpoint.key"
    ));
    let secret = load_or_create_endpoint_secret(&endpoint_secret_file)?;
    let endpoint_config = EndpointControlConfig {
        url: control_url.to_owned(),
        deployment_id,
        tenant_id: selected_tenant_id,
        principal: EndpointProofPrincipal::User { user_id },
        operation_timeout: timeout,
    };
    match AuthenticatedControlConnection::connect(&endpoint_config, &secret, connector).await {
        Ok(_) => {}
        Err(EndpointControlError::Server {
            code: ControlErrorCode::InvalidCredentials,
            ..
        }) => {
            let result = register_endpoint(
                &mut socket,
                deployment_id,
                selected_tenant_id,
                user_id,
                &secret,
                EndpointRegistration::User,
                timeout,
            )
            .await?;
            if !matches!(
                result,
                EndpointRegistrationResult::User { tenant_id, endpoint_key }
                    if tenant_id == selected_tenant_id
                        && endpoint_key == EndpointKey::new(*secret.public().as_bytes())
            ) {
                return Err(AccountScopeError::MismatchedResponse);
            }
        }
        Err(error) => return Err(error.into()),
    }
    Ok(AccountScopeRegistration {
        user_id,
        tenant_id: selected_tenant_id,
        team_name,
        endpoint_secret_file,
    })
}

type AccountSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn login(
    control_url: &str,
    username: String,
    password: Zeroizing<String>,
    connector: Connector,
    timeout: Duration,
) -> Result<((UserId, TrafficScopeOptions), AccountSocket), AccountScopeError> {
    if !control_url.starts_with("wss://") || timeout.is_zero() {
        return Err(AccountScopeError::InvalidConfig);
    }
    let request = control_url.into_client_request()?;
    let (mut socket, _) = tokio::time::timeout(
        timeout,
        connect_async_tls_with_config(request, None, false, Some(connector)),
    )
    .await
    .map_err(|_| AccountScopeError::Timeout)??;
    let login_id = RequestId::new();
    let mut message = ControlClientMessage::Login {
        request_id: login_id,
        username,
        password: password.to_string(),
    };
    send(&mut socket, &message, timeout).await?;
    if let ControlClientMessage::Login { password, .. } = &mut message {
        password.zeroize();
    }
    let (user_id, personal_tenant_id) = match receive(&mut socket, timeout).await? {
        ControlServerMessage::AccountAuthenticated {
            request_id,
            user_id,
            personal_tenant_id,
            ..
        } if request_id == login_id => (user_id, personal_tenant_id),
        response => return Err(response_error(response, login_id)),
    };
    let list_id = RequestId::new();
    send(
        &mut socket,
        &ControlClientMessage::ListTrafficScopes {
            request_id: list_id,
        },
        timeout,
    )
    .await?;
    let options = match receive(&mut socket, timeout).await? {
        ControlServerMessage::TrafficScopeList {
            request_id,
            options,
        } if request_id == list_id && options.personal_tenant_id == personal_tenant_id => options,
        response => return Err(response_error(response, list_id)),
    };
    Ok(((user_id, options), socket))
}

fn response_error(response: ControlServerMessage, request_id: RequestId) -> AccountScopeError {
    match response {
        ControlServerMessage::Error {
            request_id: Some(response_id),
            code,
            message,
        } if response_id == request_id => AccountScopeError::Server { code, message },
        _ => AccountScopeError::MismatchedResponse,
    }
}

#[derive(Debug, Error)]
pub enum AccountScopeError {
    #[error("account scope requires WSS and a nonzero timeout")]
    InvalidConfig,
    #[error("account login timed out")]
    Timeout,
    #[error("account does not belong to the selected traffic scope")]
    ScopeUnavailable,
    #[error("server returned an unexpected account scope response")]
    MismatchedResponse,
    #[error("server rejected account scope request with {code:?}: {message}")]
    Server {
        code: ControlErrorCode,
        message: String,
    },
    #[error(transparent)]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error(transparent)]
    Http(#[from] tokio_tungstenite::tungstenite::http::Error),
    #[error(transparent)]
    Enrollment(#[from] EnrollmentError),
    #[error(transparent)]
    Control(#[from] EndpointControlError),
    #[error(transparent)]
    Secret(#[from] EndpointSecretError),
}
