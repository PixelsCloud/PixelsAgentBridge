//! Read-only server checks. Never register an identity or send account credentials.
use futures_util::{SinkExt, StreamExt};
use pab_protocol::{ControlClientMessage, ControlErrorCode, ControlServerMessage, RequestId};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio_tungstenite::{
    connect_async_tls_with_config,
    tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig},
};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerKind {
    Control,
    Relay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeError {
    InvalidUrl,
    Timeout,
    Tls,
    Unreachable,
    UnexpectedResponse,
    Unhealthy,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeSuccess {
    pub elapsed_ms: u64,
}

fn address(kind: ServerKind, input: &str) -> Result<url::Url, ProbeError> {
    let url = url::Url::parse(input.trim()).map_err(|_| ProbeError::InvalidUrl)?;
    let scheme = match kind {
        ServerKind::Control => "wss",
        ServerKind::Relay => "https",
    };
    if url.scheme() != scheme
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || input.trim().chars().any(char::is_whitespace)
        || input.contains(',')
    {
        return Err(ProbeError::InvalidUrl);
    }
    Ok(url)
}

pub async fn probe(
    kind: ServerKind,
    input: &str,
    ca: Option<&[u8]>,
) -> Result<ProbeSuccess, ProbeError> {
    let url = address(kind, input)?;
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(10), check(kind, url, ca))
        .await
        .map_err(|_| ProbeError::Timeout)??;
    Ok(ProbeSuccess {
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

async fn check(kind: ServerKind, url: url::Url, ca: Option<&[u8]>) -> Result<(), ProbeError> {
    let mut builder = crate::http_client::builder(Duration::from_secs(8))
        .map_err(|_| ProbeError::Unreachable)?;
    if let Some(pem) = ca {
        let certificates =
            reqwest::Certificate::from_pem_bundle(pem).map_err(|_| ProbeError::Tls)?;
        if certificates.is_empty() {
            return Err(ProbeError::Tls);
        }
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }
    let client = builder.build().map_err(|_| ProbeError::Unreachable)?;
    let mut health = url.clone();
    health
        .set_scheme("https")
        .map_err(|_| ProbeError::InvalidUrl)?;
    health.set_query(None);
    match kind {
        ServerKind::Control => {
            health = health.join("health").map_err(|_| ProbeError::InvalidUrl)?
        }
        ServerKind::Relay => health.set_path("/healthz"),
    }
    let mut response = client.get(health).send().await.map_err(http_error)?;
    if response.status().is_server_error() {
        return Err(ProbeError::Unhealthy);
    }
    match kind {
        ServerKind::Control if response.status() != reqwest::StatusCode::NO_CONTENT => {
            return Err(ProbeError::UnexpectedResponse);
        }
        ServerKind::Relay => {
            if response.status() != reqwest::StatusCode::OK {
                return Err(ProbeError::UnexpectedResponse);
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(http_error)? {
                if body.len() + chunk.len() > 4096 {
                    return Err(ProbeError::UnexpectedResponse);
                }
                body.extend_from_slice(&chunk);
            }
            let value: serde_json::Value =
                serde_json::from_slice(&body).map_err(|_| ProbeError::UnexpectedResponse)?;
            if value["status"] != "ok" || !value["version"].as_str().is_some_and(|s| !s.is_empty())
            {
                return Err(ProbeError::UnexpectedResponse);
            }
        }
        _ => {}
    }
    let mut websocket = url;
    if matches!(kind, ServerKind::Relay) {
        websocket
            .set_scheme("wss")
            .map_err(|_| ProbeError::InvalidUrl)?;
        websocket.set_path("/relay");
        websocket.set_query(None);
    }
    let mut request = websocket
        .as_str()
        .into_client_request()
        .map_err(|_| ProbeError::InvalidUrl)?;
    if matches!(kind, ServerKind::Relay) {
        request.headers_mut().insert(
            "sec-websocket-protocol",
            "iroh-relay-v2, iroh-relay-v1".parse().unwrap(),
        );
    }
    let connector = crate::tls_connector(ca).map_err(|_| ProbeError::Tls)?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(8192))
        .max_frame_size(Some(8192));
    let (mut socket, response) =
        connect_async_tls_with_config(request, Some(config), false, Some(connector))
            .await
            .map_err(ws_error)?;
    let result = match kind {
        ServerKind::Relay => {
            match response
                .headers()
                .get("sec-websocket-protocol")
                .and_then(|h| h.to_str().ok())
            {
                Some("iroh-relay-v2" | "iroh-relay-v1") => Ok(()),
                _ => Err(ProbeError::UnexpectedResponse),
            }
        }
        ServerKind::Control => {
            let id = RequestId::new();
            let message =
                serde_json::to_string(&ControlClientMessage::ListTrafficScopes { request_id: id })
                    .map_err(|_| ProbeError::UnexpectedResponse)?;
            socket
                .send(Message::Text(message.into()))
                .await
                .map_err(ws_error)?;
            loop {
                match socket.next().await {
                    Some(Ok(Message::Text(text))) => {
                        break match serde_json::from_str::<ControlServerMessage>(&text) {
                            Ok(ControlServerMessage::Error {
                                request_id: Some(reply),
                                code: ControlErrorCode::InvalidState,
                                ..
                            }) if reply == id => Ok(()),
                            _ => Err(ProbeError::UnexpectedResponse),
                        };
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {
                        socket.flush().await.map_err(ws_error)?;
                    }
                    Some(Err(e)) => break Err(ws_error(e)),
                    _ => break Err(ProbeError::UnexpectedResponse),
                }
            }
        }
    };
    // Closing must not turn a successful check into a timeout.
    let _ = tokio::time::timeout(Duration::from_millis(200), socket.close(None)).await;
    result
}

fn http_error(error: reqwest::Error) -> ProbeError {
    if error.is_timeout() {
        return ProbeError::Timeout;
    }
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    while let Some(error) = cause {
        let message = error.to_string().to_lowercase();
        if message.contains("certificate") || message.contains("tls") {
            return ProbeError::Tls;
        }
        cause = error.source();
    }
    ProbeError::Unreachable
}

fn ws_error(error: tokio_tungstenite::tungstenite::Error) -> ProbeError {
    use tokio_tungstenite::tungstenite::Error;
    match error {
        Error::Tls(_) => ProbeError::Tls,
        Error::Io(ref e) if e.to_string().to_lowercase().contains("certificate") => ProbeError::Tls,
        Error::Io(_) => ProbeError::Unreachable,
        _ => ProbeError::UnexpectedResponse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrong_schemes_credentials_and_multiple_urls_are_rejected() {
        for input in [
            "",
            "https://control.test",
            "wss://user:secret@control.test",
            "wss://host/#fragment",
            "wss://one,wss://two",
        ] {
            assert_eq!(
                address(ServerKind::Control, input),
                Err(ProbeError::InvalidUrl)
            );
        }
        assert!(address(ServerKind::Control, " wss://control.test/control ").is_ok());
        assert!(address(ServerKind::Relay, "https://relay.test").is_ok());
        assert!(address(ServerKind::Relay, "wss://relay.test").is_err());
    }
}
