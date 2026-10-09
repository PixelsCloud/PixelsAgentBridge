//! GitHub identity proof ends here: clients receive only PAB sessions.
use super::{WebError, WebState, session};
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet,
    EndpointSet, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, TokenResponse, TokenUrl,
    basic::BasicClient,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Row, postgres::PgRow};
use std::time::Duration;
use uuid::Uuid;

type Client = BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;
const COOKIE: &str = "__Host-pab_github";
const CALLBACK: &str = "/api/account/github/callback";

// No Debug: this owns the application's secret.
pub struct GithubConfig {
    client: Client,
    http: reqwest::Client,
    origin: String,
    user_url: String,
}

impl GithubConfig {
    pub fn from_env() -> Result<Option<Self>, &'static str> {
        let Some(path) = std::env::var_os("PAB_GITHUB_CONFIG_FILE") else {
            return Ok(None);
        };
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Secret {
            client_id: String,
            client_secret: String,
        }
        let raw = std::fs::read(path).map_err(|_| "cannot read GitHub configuration")?;
        let raw = raw.strip_prefix(&[239, 187, 191]).unwrap_or(&raw);
        let secret: Secret =
            serde_json::from_slice(raw).map_err(|_| "invalid GitHub configuration")?;
        let origin =
            std::env::var("PAB_WEB_ORIGIN").map_err(|_| "GitHub login requires PAB_WEB_ORIGIN")?;
        let parsed = url::Url::parse(&origin).map_err(|_| "invalid PAB_WEB_ORIGIN")?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err("GitHub login requires an HTTPS origin without a path");
        }
        if secret.client_id.trim().is_empty()
            || secret.client_secret.trim().is_empty()
            || secret.client_secret.contains("-----BEGIN")
        {
            return Err("GitHub configuration requires Client ID and text Client Secret");
        }
        Self::new(
            secret.client_id,
            secret.client_secret,
            parsed.origin().ascii_serialization(),
            "https://github.com/login/oauth/authorize",
            "https://github.com/login/oauth/access_token",
            "https://api.github.com/user",
        )
        .map(Some)
    }

    fn new(
        id: String,
        secret: String,
        origin: String,
        auth: &str,
        token: &str,
        user: &str,
    ) -> Result<Self, &'static str> {
        let client = BasicClient::new(ClientId::new(id))
            .set_client_secret(ClientSecret::new(secret))
            .set_auth_type(AuthType::RequestBody)
            .set_auth_uri(AuthUrl::new(auth.into()).map_err(|_| "invalid GitHub authorize URL")?)
            .set_token_uri(TokenUrl::new(token.into()).map_err(|_| "invalid GitHub token URL")?)
            .set_redirect_uri(
                RedirectUrl::new(format!("{origin}{CALLBACK}"))
                    .map_err(|_| "invalid callback URL")?,
            );
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("PixelsAgentBridge")
            .build()
            .map_err(|_| "cannot create GitHub HTTP client")?;
        Ok(Self {
            client,
            http,
            origin,
            user_url: user.into(),
        })
    }

    fn authorization(&self, state: String, verifier: String) -> String {
        self.client
            .authorize_url(|| CsrfToken::new(state))
            .set_pkce_challenge(PkceCodeChallenge::from_code_verifier_sha256(
                &PkceCodeVerifier::new(verifier),
            ))
            .url()
            .0
            .into()
    }

    async fn profile(&self, code: String, verifier: String) -> Result<Profile, WebError> {
        // Bound token and profile bodies as well as time. No provider error body,
        // URL, code or token is ever included in our error/log response.
        let transport = BoundedTransport(self.http.clone());
        let token = self
            .client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(PkceCodeVerifier::new(verifier))
            .request_async(&transport)
            .await
            .map_err(|_| upstream())?;
        let response = self
            .http
            .get(&self.user_url)
            .bearer_auth(token.access_token().secret())
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|_| upstream())?;
        if !response.status().is_success() {
            return Err(upstream());
        }
        let profile: Profile =
            serde_json::from_slice(&bounded_body(response).await?).map_err(|_| upstream())?;
        if profile.id == 0 || profile.login.is_empty() || profile.login.len() > 100 {
            return Err(upstream());
        }
        Ok(profile)
    }
}

struct BoundedTransport(reqwest::Client);
impl<'c> oauth2::AsyncHttpClient<'c> for BoundedTransport {
    type Error = WebError;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<oauth2::HttpResponse, WebError>> + Send + 'c>,
    >;
    fn call(&'c self, request: oauth2::HttpRequest) -> Self::Future {
        Box::pin(async move {
            let response = self
                .0
                .execute(request.try_into().map_err(|_| upstream())?)
                .await
                .map_err(|_| upstream())?;
            let status = response.status();
            let headers = response.headers().clone();
            let mut result = oauth2::HttpResponse::new(bounded_body(response).await?);
            *result.status_mut() = status;
            *result.headers_mut() = headers;
            Ok(result)
        })
    }
}

async fn bounded_body(mut response: reqwest::Response) -> Result<Vec<u8>, WebError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| upstream())? {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err(upstream());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
fn upstream() -> WebError {
    WebError::new(StatusCode::BAD_GATEWAY, "github_unavailable")
}
fn invalid_flow() -> WebError {
    WebError::new(StatusCode::BAD_REQUEST, "github_expired")
}
fn config(state: &WebState) -> Result<&GithubConfig, WebError> {
    state
        .github
        .as_deref()
        .ok_or_else(|| WebError::new(StatusCode::NOT_FOUND, "github_disabled"))
}
fn random() -> String {
    CsrfToken::new_random_len(32).secret().to_owned()
}
fn hash(value: &str) -> String {
    session::hash_token(value)
}
fn transaction_cookie(value: &str, age: u32) -> String {
    format!("{COOKIE}={value}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={age}")
}
fn cookie(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|v| v.trim().split_once('='))
        .filter(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v);
    let value = values.next()?;
    (values.next().is_none() && (32..=128).contains(&value.len())).then_some(value)
}

#[derive(Deserialize)]
struct Profile {
    id: u64,
    login: String,
    avatar_url: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Start {
    #[serde(default)]
    bind: bool,
    redirect_uri: Option<String>,
    client_state: Option<String>,
    proof_hash: Option<String>,
}

fn valid_loopback(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|v| {
        v.scheme() == "http"
            && v.host_str() == Some("127.0.0.1")
            && v.port().is_some_and(|p| p >= 1024)
            && v.path() == "/github/callback"
            && v.username().is_empty()
            && v.password().is_none()
            && v.query().is_none()
            && v.fragment().is_none()
    })
}
fn valid_proof(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(super) async fn web_start(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(input): Json<Start>,
) -> Result<Response, WebError> {
    start(&state, &headers, input, false).await
}
pub(super) async fn native_start(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(input): Json<Start>,
) -> Result<Response, WebError> {
    start(&state, &headers, input, true).await
}
async fn start(
    state: &WebState,
    headers: &HeaderMap,
    input: Start,
    native: bool,
) -> Result<Response, WebError> {
    let config = config(state)?;
    let _permit = session::login_permit(state).await?;
    if native {
        if !input.redirect_uri.as_deref().is_some_and(valid_loopback)
            || !input.proof_hash.as_deref().is_some_and(valid_proof)
            || !input.client_state.as_deref().is_some_and(valid_proof)
        {
            return Err(WebError::invalid());
        }
    } else if input.redirect_uri.is_some()
        || input.client_state.is_some()
        || input.proof_hash.is_some()
    {
        return Err(WebError::invalid());
    }
    let me = if input.bind {
        Some(if native {
            session::native_viewer(state, headers).await?
        } else {
            session::viewer(state, headers).await?
        })
    } else {
        None
    };
    let session_hash = me.as_ref().and_then(|_| {
        if native {
            session::native_token_hash(headers)
        } else {
            session::token_hash(headers)
        }
    });
    let state_value = random();
    let verifier = PkceCodeChallenge::new_random_sha256().1.secret().clone();
    let cookie_value = random();
    let launch = native.then(random);
    let mut tx = state.control.store().pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(26035005)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM github_authorizations WHERE expires_at<now()")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM github_redemptions WHERE expires_at<now()")
        .execute(&mut *tx)
        .await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM github_authorizations")
        .fetch_one(&mut *tx)
        .await?;
    if count >= 10000 {
        return Err(WebError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited"));
    }
    sqlx::query("INSERT INTO github_authorizations(state_hash,cookie_hash,launch_hash,pkce_verifier,session_hash,user_id,auth_revision,redirect_uri,client_state,proof_hash) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(hash(&state_value)).bind((!native).then(||hash(&cookie_value))).bind(launch.as_deref().map(hash)).bind(&verifier)
        .bind(session_hash).bind(me.as_ref().map(|v| v.id)).bind(me.as_ref().map(|v|v.auth_revision))
        .bind(input.redirect_uri).bind(input.client_state).bind(input.proof_hash).execute(&mut *tx).await?;
    tx.commit().await?;
    let url = match launch {
        Some(ticket) => {
            let mut u = url::Url::parse(&format!("{}/api/account/github/authorize", config.origin))
                .map_err(|_| WebError::invalid())?;
            u.query_pairs_mut()
                .append_pair("state", &state_value)
                .append_pair("ticket", &ticket);
            u.into()
        }
        None => config.authorization(state_value, verifier),
    };
    let mut response = Json(json!({"authorization_url":url})).into_response();
    if !native {
        response.headers_mut().insert(
            header::SET_COOKIE,
            transaction_cookie(&cookie_value, 600).parse().unwrap(),
        );
    }
    Ok(response)
}

#[derive(Deserialize)]
pub(super) struct Launch {
    state: String,
    ticket: String,
}
pub(super) async fn authorize(
    State(state): State<WebState>,
    Query(input): Query<Launch>,
) -> Result<Response, WebError> {
    let config = config(&state)?;
    if input.state.len() > 128 || input.ticket.len() > 128 {
        return Err(invalid_flow());
    }
    let cookie_value = random();
    let verifier: String = sqlx::query_scalar("UPDATE github_authorizations SET cookie_hash=$1,launch_hash=NULL WHERE state_hash=$2 AND launch_hash=$3 AND expires_at>now() RETURNING pkce_verifier")
        .bind(hash(&cookie_value)).bind(hash(&input.state)).bind(hash(&input.ticket)).fetch_optional(state.control.store().pool()).await?.ok_or_else(invalid_flow)?;
    let mut response = Redirect::to(&config.authorization(input.state, verifier)).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        transaction_cookie(&cookie_value, 600).parse().unwrap(),
    );
    Ok(response)
}

#[derive(Deserialize)]
pub(super) struct Callback {
    state: Option<String>,
    code: Option<String>,
    error: Option<String>,
}
pub(super) async fn callback(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(input): Query<Callback>,
) -> Response {
    let outcome = callback_inner(&state, &headers, input).await;
    let mut response = match outcome {
        Ok(response) => response,
        Err(error) => Redirect::to(&format!("/?github_error={}", error.code)).into_response(),
    };
    response.headers_mut().append(
        header::SET_COOKIE,
        transaction_cookie("", 0).parse().unwrap(),
    );
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}
async fn callback_inner(
    state: &WebState,
    headers: &HeaderMap,
    input: Callback,
) -> Result<Response, WebError> {
    let config = config(state)?;
    let csrf = input
        .state
        .as_deref()
        .filter(|v| v.len() <= 128)
        .ok_or_else(invalid_flow)?;
    let cookie = cookie(headers).ok_or_else(invalid_flow)?;
    // Delete atomically before any outbound request. A replay cannot repeat
    // provider exchange or account changes, including when the network fails.
    let row = sqlx::query("DELETE FROM github_authorizations WHERE state_hash=$1 AND cookie_hash=$2 AND expires_at>now() RETURNING *")
        .bind(hash(&csrf)).bind(hash(cookie)).fetch_optional(state.control.store().pool()).await?.ok_or_else(invalid_flow)?;
    let result = complete_identity(state, config, &row, input).await;
    let redirect_uri: Option<String> = row.try_get("redirect_uri")?;
    if let Some(uri) = redirect_uri {
        let mut uri = url::Url::parse(&uri).map_err(|_| invalid_flow())?;
        uri.query_pairs_mut()
            .append_pair("state", &row.try_get::<String, _>("client_state")?);
        match result {
            Ok((user_id, revision)) => {
                let code = random();
                sqlx::query("INSERT INTO github_redemptions(code_hash,proof_hash,user_id,auth_revision) VALUES ($1,$2,$3,$4)")
                    .bind(hash(&code)).bind(row.try_get::<String,_>("proof_hash")?).bind(user_id).bind(revision).execute(state.control.store().pool()).await?;
                uri.query_pairs_mut().append_pair("code", &code);
            }
            Err(error) => {
                uri.query_pairs_mut().append_pair("error", error.code);
            }
        }
        return Ok(Redirect::to(uri.as_str()).into_response());
    }
    let (user, revision) = result?;
    let binding: Option<Uuid> = row.try_get("user_id")?;
    let mut response = Redirect::to(if binding.is_some() {
        "/account?github=linked"
    } else {
        "/"
    })
    .into_response();
    if binding.is_none() {
        let token = session::issue_token(state, user, revision).await?;
        response
            .headers_mut()
            .insert(header::SET_COOKIE, session::cookie(&token).parse().unwrap());
    }
    Ok(response)
}

async fn complete_identity(
    state: &WebState,
    config: &GithubConfig,
    flow: &PgRow,
    input: Callback,
) -> Result<(Uuid, i64), WebError> {
    if input.error.is_some() {
        return Err(WebError::new(StatusCode::BAD_REQUEST, "github_cancelled"));
    }
    let code = input
        .code
        .filter(|v| !v.is_empty() && v.len() <= 2048)
        .ok_or_else(invalid_flow)?;
    let profile = config.profile(code, flow.try_get("pkce_verifier")?).await?;
    identify(state, flow, profile).await
}
async fn identify(
    state: &WebState,
    flow: &PgRow,
    profile: Profile,
) -> Result<(Uuid, i64), WebError> {
    let mut tx = state.control.store().pool().begin().await?;
    let provider_id = profile.id.to_string();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,26035006))")
        .bind(&provider_id)
        .execute(&mut *tx)
        .await?;
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM external_identities WHERE provider='github' AND provider_user_id=$1",
    )
    .bind(&provider_id)
    .fetch_optional(&mut *tx)
    .await?;
    let binding: Option<Uuid> = flow.try_get("user_id")?;
    let user = if let Some(user) = binding {
        let valid: Option<i64> = sqlx::query_scalar("SELECT u.auth_revision FROM users u JOIN web_sessions s ON s.user_id=u.id WHERE u.id=$1 AND u.status='active' AND u.auth_revision=$2 AND s.token_hash=$3 FOR UPDATE OF u FOR SHARE OF s")
            .bind(user).bind(flow.try_get::<i64,_>("auth_revision")?).bind(flow.try_get::<String,_>("session_hash")?).fetch_optional(&mut *tx).await?;
        if valid.is_none() {
            return Err(WebError::unauthorized());
        }
        let linked: Option<String> = sqlx::query_scalar("SELECT provider_user_id FROM external_identities WHERE provider='github' AND user_id=$1").bind(user).fetch_optional(&mut *tx).await?;
        if existing.is_some_and(|v| v != user) || linked.is_some_and(|v| v != provider_id) {
            return Err(WebError::new(StatusCode::CONFLICT, "github_already_linked"));
        }
        user
    } else if let Some(user) = existing {
        user
    } else {
        if !state.registration_enabled {
            return Err(WebError::new(
                StatusCode::FORBIDDEN,
                "registration_disabled",
            ));
        }
        let user = Uuid::new_v4();
        let tenant = Uuid::new_v4();
        let name = format!(
            "gh-{}-{}",
            profile
                .login
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                .take(30)
                .collect::<String>(),
            &user.simple().to_string()[..16]
        );
        sqlx::query(
            "INSERT INTO users(id,username,username_key,password_hash) VALUES ($1,$2,$3,NULL)",
        )
        .bind(user)
        .bind(&name)
        .bind(name.to_lowercase())
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO tenants(id,kind,created_by_user_id) VALUES ($1,'personal',$2)")
            .bind(tenant)
            .bind(user)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO personal_tenants(tenant_id,user_id) VALUES ($1,$2)")
            .bind(tenant)
            .bind(user)
            .execute(&mut *tx)
            .await?;
        user
    };
    let revision: i64 = sqlx::query_scalar(
        "SELECT auth_revision FROM users WHERE id=$1 AND status='active' FOR UPDATE",
    )
    .bind(user)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(WebError::unauthorized)?;
    let avatar = profile.avatar_url.filter(|v| {
        v.len() <= 2048
            && url::Url::parse(v).is_ok_and(|u| {
                u.scheme() == "https" && u.host_str() == Some("avatars.githubusercontent.com")
            })
    });
    sqlx::query("INSERT INTO external_identities(provider,provider_user_id,user_id,provider_login,avatar_url) VALUES ('github',$1,$2,$3,$4) ON CONFLICT(provider,provider_user_id) DO UPDATE SET provider_login=excluded.provider_login,avatar_url=excluded.avatar_url,updated_at=now()")
        .bind(provider_id).bind(user).bind(&profile.login).bind(avatar).execute(&mut *tx).await?;
    tx.commit().await?;
    state.control.web_changed();
    Ok((user, revision))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Redeem {
    code: String,
    verifier: String,
}
pub(super) async fn redeem(
    State(state): State<WebState>,
    Json(input): Json<Redeem>,
) -> Result<Response, WebError> {
    config(&state)?;
    let _permit = session::login_permit(&state).await?;
    if input.code.len() > 128 || !valid_proof(&input.verifier) {
        return Err(invalid_flow());
    }
    let mut tx = state.control.store().pool().begin().await?;
    let row = sqlx::query("DELETE FROM github_redemptions WHERE code_hash=$1 AND proof_hash=$2 AND expires_at>now() RETURNING user_id,auth_revision")
        .bind(hash(&input.code)).bind(hash(&input.verifier)).fetch_optional(&mut *tx).await?.ok_or_else(invalid_flow)?;
    // Redeem and session insertion share a transaction; never burn a valid code
    // while failing to commit its session. Wrong proof leaves the row intact.
    let user: Uuid = row.try_get("user_id")?;
    let revision: i64 = row.try_get("auth_revision")?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .execute(&mut *tx)
        .await?;
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let inserted = sqlx::query("INSERT INTO web_sessions(token_hash,user_id) SELECT $1,id FROM users WHERE id=$2 AND auth_revision=$3 AND status='active'")
        .bind(hash(&token)).bind(user).bind(revision).execute(&mut *tx).await?;
    if inserted.rows_affected() != 1 {
        return Err(WebError::unauthorized());
    }
    tx.commit().await?;
    let viewer = session::viewer_by_hash(&state, &hash(&token)).await?;
    Ok(Json(json!({"access_token":token,"user":viewer})).into_response())
}

async fn status(state: &WebState, me: session::Viewer) -> Result<Json<Value>, WebError> {
    let row = sqlx::query("SELECT u.password_hash IS NOT NULL AS password_enabled, e.provider_login FROM users u LEFT JOIN external_identities e ON e.user_id=u.id AND e.provider='github' WHERE u.id=$1").bind(me.id).fetch_one(state.control.store().pool()).await?;
    Ok(Json(
        json!({"enabled":state.github.is_some(),"login":row.try_get::<Option<String>,_>("provider_login")?,"password_enabled":row.try_get::<bool,_>("password_enabled")?}),
    ))
}
pub(super) async fn web_status(
    State(s): State<WebState>,
    h: HeaderMap,
) -> Result<Json<Value>, WebError> {
    let me = session::viewer(&s, &h).await?;
    status(&s, me).await
}
pub(super) async fn native_status(
    State(s): State<WebState>,
    h: HeaderMap,
) -> Result<Json<Value>, WebError> {
    let me = session::native_viewer(&s, &h).await?;
    status(&s, me).await
}
async fn unlink(state: &WebState, me: session::Viewer) -> Result<StatusCode, WebError> {
    let mut tx = state.control.store().pool().begin().await?;
    let identity: Option<String> = sqlx::query_scalar(
        "SELECT provider_user_id FROM external_identities WHERE provider='github' AND user_id=$1",
    )
    .bind(me.id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(id) = identity.as_ref() {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,26035006))")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    let password: bool = sqlx::query_scalar("SELECT password_hash IS NOT NULL FROM users WHERE id=$1 AND status='active' AND auth_revision=$2 FOR UPDATE").bind(me.id).bind(me.auth_revision).fetch_optional(&mut *tx).await?.ok_or_else(WebError::unauthorized)?;
    if !password {
        return Err(WebError::new(StatusCode::CONFLICT, "last_login_method"));
    }
    let current: Option<String> = sqlx::query_scalar(
        "SELECT provider_user_id FROM external_identities WHERE provider='github' AND user_id=$1",
    )
    .bind(me.id)
    .fetch_optional(&mut *tx)
    .await?;
    if current != identity {
        return Err(WebError::new(StatusCode::CONFLICT, "conflict"));
    }
    sqlx::query("DELETE FROM external_identities WHERE provider='github' AND user_id=$1")
        .bind(me.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE users SET auth_revision=auth_revision+1 WHERE id=$1")
        .bind(me.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    state.control.web_changed();
    Ok(StatusCode::NO_CONTENT)
}
pub(super) async fn web_unlink(
    State(s): State<WebState>,
    h: HeaderMap,
) -> Result<StatusCode, WebError> {
    let me = session::viewer(&s, &h).await?;
    unlink(&s, me).await
}
pub(super) async fn native_unlink(
    State(s): State<WebState>,
    h: HeaderMap,
) -> Result<StatusCode, WebError> {
    let me = session::native_viewer(&s, &h).await?;
    unlink(&s, me).await
}

#[cfg(test)]
mod tests;
