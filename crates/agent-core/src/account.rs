//! HTTP user sessions are independent of endpoint/device authentication.
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};
use zeroize::Zeroizing;

pub mod local_device;
#[cfg(target_os = "macos")]
mod macos_credentials;
mod store;
pub use store::{AccountState, AccountStore};

fn notifications() -> &'static tokio::sync::watch::Sender<u64> {
    static CHANNEL: std::sync::OnceLock<tokio::sync::watch::Sender<u64>> =
        std::sync::OnceLock::new();
    CHANNEL.get_or_init(|| tokio::sync::watch::channel(0).0)
}

pub fn account_changes() -> tokio::sync::watch::Receiver<u64> {
    notifications().subscribe()
}

/// A hint only: consumers always read the protected local store, never an IPC identity.
pub fn notify_account_change(revision: u64) {
    notifications().send_if_modified(|current| {
        if revision > *current {
            *current = revision;
            true
        } else {
            false
        }
    });
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountUser {
    pub id: pab_protocol::UserId,
    pub username: String,
    pub server_admin: bool,
}

// Deliberately no Debug: responses contain bearer credentials.
#[derive(Deserialize)]
pub struct AccountSession {
    pub user: AccountUser,
    pub access_token: String,
}

#[derive(Clone)]
pub struct AccountClient {
    client: reqwest::Client,
    origin: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("account server URL must use HTTPS or WSS without credentials")]
    InvalidUrl,
    #[error("account request failed; cached login was retained")]
    Network,
    #[error("account service returned HTTP {0}")]
    Http(u16),
    #[error("account service returned an invalid response")]
    InvalidResponse,
    #[error("could not access the current user's account store")]
    Storage,
}

pub fn account_origin(control_url: &str) -> Result<String, AccountError> {
    let mut url = url::Url::parse(control_url).map_err(|_| AccountError::InvalidUrl)?;
    if !matches!(url.scheme(), "https" | "wss")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(AccountError::InvalidUrl);
    }
    url.set_scheme("https")
        .map_err(|_| AccountError::InvalidUrl)?;
    Ok(url.origin().ascii_serialization())
}

impl AccountClient {
    pub fn new(control_url: &str, ca_pem: Option<&[u8]>) -> Result<Self, AccountError> {
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none());
        if let Some(pem) = ca_pem {
            builder = builder.add_root_certificate(
                reqwest::Certificate::from_pem(pem).map_err(|_| AccountError::InvalidResponse)?,
            );
        }
        Ok(Self {
            client: builder.build().map_err(|_| AccountError::Network)?,
            origin: account_origin(control_url)?,
        })
    }

    pub fn from_env() -> Result<Self, AccountError> {
        let url = std::env::var("PAB_CONTROL_URL").map_err(|_| AccountError::InvalidUrl)?;
        let ca = std::env::var_os("PAB_CONTROL_CA_CERT")
            .map(|path| std::fs::read(Path::new(&path)))
            .transpose()
            .map_err(|_| AccountError::Storage)?;
        Self::new(&url, ca.as_deref())
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub async fn login(
        &self,
        username: &str,
        password: Zeroizing<String>,
        register: bool,
    ) -> Result<AccountSession, AccountError> {
        let route = if register { "register" } else { "session" };
        let response = self
            .client
            .post(format!("{}/api/account/{route}", self.origin))
            .json(&serde_json::json!({"username":username,"password":password.as_str()}))
            .send()
            .await
            .map_err(|_| AccountError::Network)?;
        let response = checked(response)?;
        let session: AccountSession = response
            .json()
            .await
            .map_err(|_| AccountError::InvalidResponse)?;
        if session.access_token.len() != 64
            || !session.access_token.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(AccountError::InvalidResponse);
        }
        Ok(session)
    }

    pub async fn current(&self, token: &str) -> Result<AccountUser, AccountError> {
        checked(
            self.client
                .get(format!("{}/api/account/session", self.origin))
                .bearer_auth(token)
                .send()
                .await
                .map_err(|_| AccountError::Network)?,
        )?
        .json()
        .await
        .map_err(|_| AccountError::InvalidResponse)
    }

    pub async fn device_association_challenge(
        &self,
        token: &str,
        device: pab_protocol::DeviceId,
        input: &pab_protocol::DeviceAccountChallengeRequest,
    ) -> Result<pab_protocol::DeviceAccountState, AccountError> {
        checked(
            self.client
                .post(format!(
                    "{}/api/account/devices/{device}/association-challenge",
                    self.origin
                ))
                .bearer_auth(token)
                .json(input)
                .send()
                .await
                .map_err(|_| AccountError::Network)?,
        )?
        .json()
        .await
        .map_err(|_| AccountError::InvalidResponse)
    }

    pub async fn saved_devices(
        &self,
        token: &str,
        after: i64,
    ) -> Result<pab_protocol::SavedDeviceChanges, AccountError> {
        checked(
            self.client
                .get(format!(
                    "{}/api/account/saved-devices?after={after}",
                    self.origin
                ))
                .bearer_auth(token)
                .send()
                .await
                .map_err(|_| AccountError::Network)?,
        )?
        .json()
        .await
        .map_err(|_| AccountError::InvalidResponse)
    }

    pub async fn update_saved_device(
        &self,
        token: &str,
        input: &pab_protocol::SavedDeviceMutation,
    ) -> Result<i64, AccountError> {
        let value: serde_json::Value = checked(
            self.client
                .post(format!("{}/api/account/saved-devices", self.origin))
                .bearer_auth(token)
                .json(input)
                .send()
                .await
                .map_err(|_| AccountError::Network)?,
        )?
        .json()
        .await
        .map_err(|_| AccountError::InvalidResponse)?;
        value["revision"]
            .as_i64()
            .filter(|v| *v > 0)
            .ok_or(AccountError::InvalidResponse)
    }
    pub async fn import_saved_device(
        &self,
        token: &str,
        input: &pab_protocol::SavedDeviceImport,
    ) -> Result<(), AccountError> {
        checked(
            self.client
                .post(format!("{}/api/account/saved-devices/import", self.origin))
                .bearer_auth(token)
                .json(input)
                .send()
                .await
                .map_err(|_| AccountError::Network)?,
        )?;
        Ok(())
    }

    pub async fn report_usage(
        &self,
        token: &str,
        batch: &pab_protocol::UsageBatch,
    ) -> Result<(), AccountError> {
        checked(
            self.client
                .post(format!("{}/api/account/usage", self.origin))
                .bearer_auth(token)
                .json(batch)
                .send()
                .await
                .map_err(|_| AccountError::Network)?,
        )?;
        Ok(())
    }

    pub async fn associate_device(
        &self,
        token: &str,
        device: pab_protocol::DeviceId,
        proof: &pab_protocol::DeviceAccountProof,
    ) -> Result<pab_protocol::DeviceAccountState, AccountError> {
        checked(
            self.client
                .put(format!(
                    "{}/api/account/devices/{device}/association",
                    self.origin
                ))
                .bearer_auth(token)
                .json(proof)
                .send()
                .await
                .map_err(|_| AccountError::Network)?,
        )?
        .json()
        .await
        .map_err(|_| AccountError::InvalidResponse)
    }

    /// A successful HTTP response alone must not be reported as a saved login.
    /// Revoke the new session if local credential persistence fails.
    pub async fn save_session(
        &self,
        store: &AccountStore,
        session: AccountSession,
    ) -> Result<AccountState, AccountError> {
        let token = Zeroizing::new(session.access_token.clone());
        match store.login(session) {
            Ok(state) => Ok(state),
            Err(error) => {
                let _ = self.logout(&token).await;
                Err(error)
            }
        }
    }

    pub async fn logout(&self, token: &str) -> Result<(), AccountError> {
        checked(
            self.client
                .post(format!("{}/api/account/logout", self.origin))
                .bearer_auth(token)
                .send()
                .await
                .map_err(|_| AccountError::Network)?,
        )?;
        Ok(())
    }

    pub async fn bind_endpoint(
        &self,
        secret: &iroh_base::SecretKey,
        revision: u64,
        token: Option<&str>,
    ) -> Result<pab_protocol::EndpointUserContextReceipt, AccountError> {
        let mut update = pab_protocol::EndpointUserContextUpdate {
            endpoint_key: pab_protocol::EndpointKey::new(*secret.public().as_bytes()),
            revision,
            issued_at_unix_ms: (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(i64::MAX as u128)) as i64,
            signature: pab_protocol::EndpointSignature::from_bytes([0; 64]),
        };
        let digest = token
            .map(|token| blake3::hash(token.as_bytes()).to_hex().to_string())
            .unwrap_or_default();
        update.signature = pab_protocol::EndpointSignature::from_bytes(
            secret
                .sign(&update.signing_message(&self.origin, &digest))
                .to_bytes(),
        );
        let mut request = self
            .client
            .put(format!("{}/api/account/endpoint-context", self.origin))
            .json(&update);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        checked(request.send().await.map_err(|_| AccountError::Network)?)?
            .json()
            .await
            .map_err(|_| AccountError::InvalidResponse)
    }

    /// Retry explicit offline logout tombstones without restoring an old identity.
    pub async fn flush_logouts(&self, store: &AccountStore) -> Result<(), AccountError> {
        for slot in store.read()?.pending_logouts {
            if let Some(token) = store.token(&slot)? {
                match self.logout(&token).await {
                    Ok(()) | Err(AccountError::Http(401)) => {}
                    Err(error) => return Err(error),
                }
            }
            store.finish_logout(&slot)?;
        }
        Ok(())
    }
}

fn checked(response: reqwest::Response) -> Result<reqwest::Response, AccountError> {
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(AccountError::Http(response.status().as_u16()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origins_scope_credentials_and_reject_insecure_or_embedded_secrets() {
        assert_eq!(
            account_origin("wss://example.com:8443/control").unwrap(),
            "https://example.com:8443"
        );
        assert_eq!(
            account_origin("https://EXAMPLE.com/control").unwrap(),
            "https://example.com"
        );
        for invalid in [
            "http://example.com",
            "ws://example.com",
            "https://user:secret@example.com",
            "https://example.com/#x",
        ] {
            assert!(account_origin(invalid).is_err());
        }
        assert_ne!(
            account_origin("https://one.example").unwrap(),
            account_origin("https://two.example").unwrap()
        );
    }
}
