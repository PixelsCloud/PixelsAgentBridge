use super::{AccountClient, AccountError, AccountSession};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct GithubStatus {
    pub enabled: bool,
    pub login: Option<String>,
    pub password_enabled: bool,
}
#[derive(Deserialize)]
pub struct GithubStart {
    pub authorization_url: String,
}

impl AccountClient {
    pub async fn github_enabled(&self) -> Result<bool, AccountError> {
        let value: serde_json::Value = github_checked(
            self.client
                .get(format!("{}/api/account/config", self.origin))
                .send()
                .await
                .map_err(|_| AccountError::Network)?,
        )
        .await?
        .json()
        .await
        .map_err(|_| AccountError::InvalidResponse)?;
        Ok(value["github_enabled"].as_bool().unwrap_or(false))
    }
    pub async fn github_start(
        &self,
        token: Option<&str>,
        redirect_uri: &str,
        client_state: &str,
        verifier: &str,
    ) -> Result<GithubStart, AccountError> {
        let mut request = self.client.post(format!("{}/api/account/github/start", self.origin)).json(&serde_json::json!({"bind":token.is_some(),"redirect_uri":redirect_uri,"client_state":client_state,"proof_hash":blake3::hash(verifier.as_bytes()).to_hex().to_string()}));
        if let Some(token) = token {
            request = request.bearer_auth(token)
        }
        let result: GithubStart =
            github_checked(request.send().await.map_err(|_| AccountError::Network)?)
                .await?
                .json()
                .await
                .map_err(|_| AccountError::InvalidResponse)?;
        let url = url::Url::parse(&result.authorization_url)
            .map_err(|_| AccountError::InvalidResponse)?;
        if url.origin().ascii_serialization() != self.origin
            || url.path() != "/api/account/github/authorize"
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(AccountError::InvalidResponse);
        }
        Ok(result)
    }
    pub async fn github_redeem(
        &self,
        code: &str,
        verifier: &str,
    ) -> Result<AccountSession, AccountError> {
        let result: AccountSession = github_checked(
            self.client
                .post(format!("{}/api/account/github/redeem", self.origin))
                .json(&serde_json::json!({"code":code,"verifier":verifier}))
                .send()
                .await
                .map_err(|_| AccountError::Network)?,
        )
        .await?
        .json()
        .await
        .map_err(|_| AccountError::InvalidResponse)?;
        if result.access_token.len() != 64
            || !result.access_token.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(AccountError::InvalidResponse);
        }
        Ok(result)
    }
    pub async fn github_status(&self, token: &str) -> Result<GithubStatus, AccountError> {
        github_checked(
            self.client
                .get(format!("{}/api/account/github", self.origin))
                .bearer_auth(token)
                .send()
                .await
                .map_err(|_| AccountError::Network)?,
        )
        .await?
        .json()
        .await
        .map_err(|_| AccountError::InvalidResponse)
    }
    pub async fn github_unlink(&self, token: &str) -> Result<(), AccountError> {
        github_checked(
            self.client
                .delete(format!("{}/api/account/github", self.origin))
                .bearer_auth(token)
                .send()
                .await
                .map_err(|_| AccountError::Network)?,
        )
        .await?;
        Ok(())
    }
    pub async fn discard_session(&self, token: &str) -> Result<(), AccountError> {
        github_checked(
            self.client
                .post(format!("{}/api/account/logout", self.origin))
                .bearer_auth(token)
                .send()
                .await
                .map_err(|_| AccountError::Network)?,
        )
        .await?;
        Ok(())
    }
}

async fn github_checked(response: reqwest::Response) -> Result<reqwest::Response, AccountError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let value: serde_json::Value = response.json().await.unwrap_or_default();
    let code = match value["code"].as_str() {
        Some("github_expired") => "github_expired",
        Some("github_unavailable") => "github_unavailable",
        Some("github_disabled") => "github_disabled",
        Some("github_already_linked") => "github_already_linked",
        Some("last_login_method") => "last_login_method",
        Some("registration_disabled") => "registration_disabled",
        _ => return Err(AccountError::Http(status)),
    };
    Err(AccountError::Github(code))
}
