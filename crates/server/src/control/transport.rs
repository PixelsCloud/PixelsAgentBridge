use std::{net::SocketAddr, path::PathBuf, time::Duration};

use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use pab_protocol::{ControlErrorCode, ControlServerMessage};

use super::{ControlApiState, ControlSession, relay_session::run_relay_socket};

const MAX_CONTROL_MESSAGE_BYTES: usize = 64 * 1024;
const LOGIN_MESSAGE_TIMEOUT: Duration = Duration::from_secs(20);
const AUTHENTICATED_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

pub fn control_router(state: ControlApiState) -> Router {
    let assets = crate::web::assets(state.config.web_assets.clone());
    let web = crate::web::router(state.clone());
    Router::new()
        .route("/health", get(health))
        .route("/control", get(control_upgrade))
        .route("/relay-control", get(relay_control_upgrade))
        .with_state(state)
        .merge(web)
        .fallback_service(assets)
        .layer(axum::middleware::from_fn(crate::web::response_headers))
}

pub async fn serve_tls(
    address: SocketAddr,
    certificate_path: PathBuf,
    private_key_path: PathBuf,
    state: ControlApiState,
) -> std::io::Result<()> {
    serve_tls_until(
        address,
        certificate_path,
        private_key_path,
        state,
        std::future::pending::<()>(),
    )
    .await
}

pub async fn serve_tls_until(
    address: SocketAddr,
    certificate_path: PathBuf,
    private_key_path: PathBuf,
    state: ControlApiState,
    stop: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tls =
        axum_server::tls_rustls::RustlsConfig::from_pem_file(certificate_path, private_key_path)
            .await?;
    let handle = axum_server::Handle::new();
    let server = axum_server::bind_rustls(address, tls)
        .handle(handle.clone())
        .serve(control_router(state).into_make_service());
    tokio::pin!(server);
    tokio::select! {
        result=&mut server=>result,
        _=stop=>{
            handle.graceful_shutdown(Some(Duration::from_secs(5)));
            tokio::time::timeout(Duration::from_secs(7),server).await
                .unwrap_or_else(|_|Err(std::io::Error::new(std::io::ErrorKind::TimedOut,"server stop deadline exceeded")))
        }
    }
}

async fn health(State(state): State<ControlApiState>) -> StatusCode {
    match sqlx::query("SELECT 1")
        .execute(state.control.store().pool())
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

async fn control_upgrade(
    State(state): State<ControlApiState>,
    websocket: WebSocketUpgrade,
) -> Response {
    websocket
        .max_message_size(MAX_CONTROL_MESSAGE_BYTES)
        .max_frame_size(MAX_CONTROL_MESSAGE_BYTES)
        .on_upgrade(move |socket| run_socket(socket, state))
}

async fn relay_control_upgrade(
    State(state): State<ControlApiState>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response {
    let authorization = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let identity = if let Some(config) = state.control.relay_config() {
        super::RelayControlAuth::cluster_identity(authorization, &config).map(Some)
    } else {
        state.relay_auth.identity(authorization)
    };
    let Some(identity) = identity else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let binding = *blake3::hash(
        authorization
            .unwrap_or_default()
            .strip_prefix("Bearer ")
            .unwrap_or_default()
            .as_bytes(),
    )
    .as_bytes();
    websocket
        .max_message_size(2 * 1024 * 1024 + 16 * 1024)
        .max_frame_size(2 * 1024 * 1024 + 16 * 1024)
        .on_upgrade(move |socket| run_relay_socket(socket, state, identity, binding))
}

async fn run_socket(socket: WebSocket, state: ControlApiState) {
    let (mut sender, mut receiver) = socket.split();
    let mut session = ControlSession::new((*state.control).clone(), state.config);

    loop {
        let idle_timeout = if session.is_authenticated() {
            AUTHENTICATED_IDLE_TIMEOUT
        } else {
            LOGIN_MESSAGE_TIMEOUT
        };
        let next = tokio::time::timeout(idle_timeout, receiver.next()).await;
        let message = match next {
            Ok(Some(Ok(message))) => message,
            Ok(Some(Err(_))) | Ok(None) | Err(_) => break,
        };
        let response = match message {
            Message::Text(text) => match serde_json::from_str(text.as_str()) {
                Ok(message) => session.handle(message).await,
                Err(_) => ControlServerMessage::Error {
                    request_id: None,
                    code: ControlErrorCode::InvalidMessage,
                    message: "invalid control message".to_owned(),
                },
            },
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => break,
            Message::Binary(_) => ControlServerMessage::Error {
                request_id: None,
                code: ControlErrorCode::InvalidMessage,
                message: "control messages must use text JSON frames".to_owned(),
            },
        };
        let Ok(encoded) = serde_json::to_string(&response) else {
            break;
        };
        if sender.send(Message::Text(encoded.into())).await.is_err() {
            break;
        }
    }
}
