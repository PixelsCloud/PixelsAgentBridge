//! Temporary loopback listener, separate from the public MCP status service.
use crate::operator::{OperatorState, ScopeStatus, account_status};
use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use pab_agent_core::account::{AccountClient, AccountSession, AccountStore, GithubStatus};
use serde::Deserialize;
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, oneshot};

struct Pending {
    id: String,
    cancel: oneshot::Sender<()>,
}
#[derive(Default)]
pub struct GithubLoginState {
    pending: Mutex<Option<Pending>>,
}

fn random() -> String {
    format!(
        "{}{}",
        pab_protocol::RequestId::new(),
        pab_protocol::RequestId::new()
    )
    .replace('-', "")
}
fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tauri::command]
pub async fn github_enabled() -> Result<bool, String> {
    AccountClient::from_env()
        .map_err(text)?
        .github_enabled()
        .await
        .map_err(text)
}

fn stored_token(store: &AccountStore) -> Result<zeroize::Zeroizing<String>, String> {
    let state = store.read().map_err(text)?;
    store
        .token(state.active_slot.as_deref().ok_or("unauthorized")?)
        .map_err(text)?
        .ok_or("unauthorized".into())
}
#[tauri::command]
pub async fn github_status() -> Result<GithubStatus, String> {
    let store = AccountStore::from_env().map_err(text)?;
    let token = stored_token(&store)?;
    AccountClient::from_env()
        .map_err(text)?
        .github_status(&token)
        .await
        .map_err(text)
}
#[tauri::command]
pub async fn github_unlink(operator: tauri::State<'_, OperatorState>) -> Result<(), String> {
    let _guard = operator.account_changes.lock().await;
    let store = AccountStore::from_env().map_err(text)?;
    let token = stored_token(&store)?;
    AccountClient::from_env()
        .map_err(text)?
        .github_unlink(&token)
        .await
        .map_err(text)
}
#[tauri::command]
pub async fn github_cancel(state: tauri::State<'_, GithubLoginState>) -> Result<(), String> {
    if let Some(pending) = state.pending.lock().await.take() {
        let _ = pending.cancel.send(());
    }
    Ok(())
}
#[tauri::command]
pub async fn github_login(
    state: tauri::State<'_, GithubLoginState>,
    operator: tauri::State<'_, OperatorState>,
    reporting: tauri::State<'_, crate::mcp_reporting::McpReportingState>,
    bind: bool,
) -> Result<ScopeStatus, String> {
    let id = random();
    let (cancel, cancelled) = oneshot::channel();
    {
        let mut pending = state.pending.lock().await;
        if pending.is_some() {
            return Err("github_busy".into());
        }
        *pending = Some(Pending {
            id: id.clone(),
            cancel,
        });
    }
    let result = login(&state, &operator, &reporting, &id, bind, cancelled).await;
    let mut pending = state.pending.lock().await;
    if pending.as_ref().is_some_and(|v| v.id == id) {
        pending.take();
    }
    result
}
async fn login(
    state: &GithubLoginState,
    operator: &OperatorState,
    reporting: &crate::mcp_reporting::McpReportingState,
    id: &str,
    bind: bool,
    cancelled: oneshot::Receiver<()>,
) -> Result<ScopeStatus, String> {
    let client = AccountClient::from_env().map_err(text)?;
    let store = AccountStore::from_env().map_err(text)?;
    let before = store.read().map_err(text)?;
    let token = if bind {
        Some(stored_token(&store)?)
    } else {
        None
    };
    let session = tokio::select! {
        result=browser_flow(&client,token.as_deref().map(String::as_str))=>result?,
        _=cancelled=>return Err("github_cancelled".into()),
        _=tokio::time::sleep(Duration::from_secs(600))=>return Err("github_expired".into()),
    };
    let _guard = operator.account_changes.lock().await;
    let pending = state.pending.lock().await;
    if !pending.as_ref().is_some_and(|v| v.id == id)
        || store.read().map_err(text)?.revision != before.revision
        || (bind && before.user.as_ref().map(|u| u.id) != Some(session.user.id))
    {
        let _ = client.discard_session(&session.access_token).await;
        return Err("github_cancelled".into());
    }
    if bind {
        // Linking doesn't replace the current PAB session or restart connections.
        let _ = client.discard_session(&session.access_token).await;
        return account_status(&before).ok_or("unauthorized".into());
    }
    let snapshot = client.save_session(&store, session).await.map_err(text)?;
    reporting.account_changed(snapshot.revision);
    pab_agent_core::account::notify_account_change(snapshot.revision);
    tokio::spawn(async move {
        let _ = client.flush_logouts(&store).await;
    });
    account_status(&snapshot).ok_or("account state was not saved".into())
}

#[derive(Clone)]
struct ListenerState {
    nonce: String,
    host: String,
    sender: Arc<Mutex<Option<oneshot::Sender<Result<String, String>>>>>,
}
#[derive(Deserialize)]
struct Callback {
    state: Option<String>,
    code: Option<String>,
    error: Option<String>,
}
async fn callback(
    State(state): State<ListenerState>,
    headers: HeaderMap,
    Query(input): Query<Callback>,
) -> Response {
    if headers.get(header::HOST).and_then(|h| h.to_str().ok()) != Some(state.host.as_str())
        || input.state.as_deref() != Some(state.nonce.as_str())
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let outcome = match (input.code, input.error) {
        (Some(code), None) if !code.is_empty() && code.len() <= 128 => Ok(code),
        (None, Some(error))
            if [
                "github_cancelled",
                "github_unavailable",
                "github_already_linked",
                "registration_disabled",
                "unauthorized",
            ]
            .contains(&error.as_str()) =>
        {
            Err(error)
        }
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    let Some(sender) = state.sender.lock().await.take() else {
        return StatusCode::GONE.into_response();
    };
    let _ = sender.send(outcome);
    ([(header::CACHE_CONTROL,"no-store"),(header::REFERRER_POLICY,"no-referrer"),(header::CONTENT_SECURITY_POLICY,"default-src 'none'; frame-ancestors 'none'")],Html("<!doctype html><meta charset=utf-8><title>Pixels Agent Bridge</title><p>请返回 Pixels Agent Bridge 查看登录结果。You can return to Pixels Agent Bridge.</p>")).into_response()
}
// Dropping the command on cancellation/timeout also stops its HTTP listener.
struct ListenerTask(tokio::task::JoinHandle<()>);
impl Drop for ListenerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn browser_flow(
    client: &AccountClient,
    token: Option<&str>,
) -> Result<AccountSession, String> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|_| "github_listener_failed")?;
    let address = listener
        .local_addr()
        .map_err(|_| "github_listener_failed")?;
    let nonce = random();
    let verifier = zeroize::Zeroizing::new(random());
    let (sender, result) = oneshot::channel();
    let app = Router::new()
        .route("/github/callback", get(callback))
        .with_state(ListenerState {
            nonce: nonce.clone(),
            host: address.to_string(),
            sender: Arc::new(Mutex::new(Some(sender))),
        });
    let _listener = ListenerTask(tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    }));
    let start = client
        .github_start(
            token,
            &format!("http://{address}/github/callback"),
            &nonce,
            &verifier,
        )
        .await
        .map_err(text)?;
    webbrowser::open(&start.authorization_url).map_err(|_| "github_browser_failed")?;
    let code = result.await.map_err(|_| "github_cancelled")??;
    client.github_redeem(&code, &verifier).await.map_err(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn callback_checks_host_and_state_before_consuming_code() {
        let (sender, mut received) = oneshot::channel();
        let state = ListenerState {
            nonce: "fixture-nonce".into(),
            host: "127.0.0.1:49001".into(),
            sender: Arc::new(Mutex::new(Some(sender))),
        };
        let input = || Callback {
            state: Some("fixture-nonce".into()),
            code: Some("fixture-code".into()),
            error: None,
        };
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "evil.example".parse().unwrap());
        assert_eq!(
            callback(State(state.clone()), headers.clone(), Query(input()))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        headers.insert(header::HOST, "127.0.0.1:49001".parse().unwrap());
        let mut bad = input();
        bad.state = Some("wrong".into());
        assert_eq!(
            callback(State(state.clone()), headers.clone(), Query(bad))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert!(received.try_recv().is_err());
        assert_eq!(
            callback(State(state.clone()), headers.clone(), Query(input()))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(received.await.unwrap().unwrap(), "fixture-code");
        assert_eq!(
            callback(State(state), headers, Query(input()))
                .await
                .status(),
            StatusCode::GONE
        );
    }
    #[tokio::test]
    async fn dropping_flow_releases_loopback_port() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = ListenerTask(tokio::spawn(async move {
            let _listener = listener;
            std::future::pending::<()>().await;
        }));
        drop(task);
        tokio::task::yield_now().await;
        let rebound = tokio::net::TcpListener::bind(address).await.unwrap();
        drop(rebound);
    }
}
