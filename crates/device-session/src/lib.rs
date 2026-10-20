//! Shared device password gate. Platform execution starts only after this gate.
#![forbid(unsafe_code)]

pub mod port_forward;

use argon2::{Argon2, PasswordHash, PasswordVerifier};
use pab_agent_core::{DevicePeerAuthorizer, PeerAuthorizationError};
use pab_protocol::{
    AuthorizedDevicePeer, DEVICE_SESSION_AUTH_SCHEMA_VERSION, DeviceRef, DeviceSessionAuthenticate,
    DeviceSessionAuthenticationResult, EndpointKey, MAX_DEVICE_PASSWORD_BYTES,
};
use pab_transport::{PabConnection, PabConnectionError};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct DevicePassword {
    version: u64,
    hash: String,
}

impl DevicePassword {
    pub fn from_phc(version: u64, hash: String) -> Result<Self, SessionError> {
        let parsed = PasswordHash::new(&hash).map_err(|_| SessionError::InvalidCredential)?;
        if version == 0 || parsed.algorithm.as_str() != "argon2id" {
            return Err(SessionError::InvalidCredential);
        }
        Ok(Self { version, hash })
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub async fn verify(&self, password: Zeroizing<String>) -> Result<bool, SessionError> {
        let hash = self.hash.clone();
        tokio::task::spawn_blocking(move || {
            let parsed = PasswordHash::new(&hash).map_err(|_| SessionError::InvalidCredential)?;
            Ok(Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok())
        })
        .await
        .map_err(|_| SessionError::PasswordWorker)?
    }
}

#[derive(Clone, Default)]
pub struct PasswordAttempts(Arc<Mutex<HashMap<EndpointKey, Vec<Instant>>>>);

/// Counts distinct authenticated operators, including concurrent connections per key.
#[derive(Clone, Default)]
pub struct ActiveSessions(Arc<StdMutex<HashMap<EndpointKey, u16>>>);

impl ActiveSessions {
    pub fn enter(&self, endpoint_key: EndpointKey) -> ActiveSessionLease {
        let mut sessions = self.0.lock().unwrap();
        *sessions.entry(endpoint_key).or_default() += 1;
        ActiveSessionLease {
            sessions: self.clone(),
            endpoint_key,
        }
    }

    pub fn operator_count(&self) -> u16 {
        self.0.lock().unwrap().len() as u16
    }
}

pub struct ActiveSessionLease {
    sessions: ActiveSessions,
    endpoint_key: EndpointKey,
}

impl Drop for ActiveSessionLease {
    fn drop(&mut self) {
        let mut sessions = self.sessions.0.lock().unwrap();
        if let Some(count) = sessions.get_mut(&self.endpoint_key) {
            *count -= 1;
            if *count == 0 {
                sessions.remove(&self.endpoint_key);
            }
        }
    }
}

/// The same gate is used by desktop Executor and Android, including lockout policy.
pub async fn authenticate(
    connection: &PabConnection,
    authorizer: &DevicePeerAuthorizer,
    credential: &DevicePassword,
    device_ref: DeviceRef,
    timeout: Duration,
    failures: &PasswordAttempts,
) -> Result<AuthorizedDevicePeer, SessionError> {
    if timeout.is_zero() {
        return Err(SessionError::InvalidTimeout);
    }
    let mut stream = connection.accept_bi(timeout).await?;
    let request: DeviceSessionAuthenticate = stream.receive_sensitive_json(timeout).await?;
    let password = Zeroizing::new(request.device_password);
    if request.schema_version != DEVICE_SESSION_AUTH_SCHEMA_VERSION
        || request.device_ref != device_ref
        || password.is_empty()
        || password.len() > MAX_DEVICE_PASSWORD_BYTES
    {
        reject(&mut stream, timeout).await?;
        return Err(SessionError::Rejected);
    }
    let peer_key = EndpointKey::new(connection.remote_endpoint_key());
    let authorized = match authorizer.authorize(peer_key).await {
        Ok(peer)
            if peer.device_ref == device_ref
                && peer.peer_endpoint_key == peer_key
                && peer.operator.endpoint_key() == peer_key =>
        {
            peer
        }
        Ok(_) => {
            reject(&mut stream, timeout).await?;
            return Err(SessionError::IdentityMismatch);
        }
        Err(error) => {
            reject(&mut stream, timeout).await?;
            return Err(SessionError::Authorization(error));
        }
    };
    let locked = {
        let mut map = failures.0.lock().await;
        let now = Instant::now();
        map.retain(|_, attempts| {
            attempts.retain(|at| now.duration_since(*at) < Duration::from_secs(600));
            !attempts.is_empty()
        });
        map.get(&peer_key)
            .is_some_and(|attempts| attempts.len() >= 5)
    };
    if locked {
        reject(&mut stream, timeout).await?;
        return Err(SessionError::Rejected);
    }
    if !credential.verify(password).await? {
        failures
            .0
            .lock()
            .await
            .entry(peer_key)
            .or_default()
            .push(Instant::now());
        tokio::time::sleep(Duration::from_millis(500)).await;
        reject(&mut stream, timeout).await?;
        return Err(SessionError::Rejected);
    }
    failures.0.lock().await.remove(&peer_key);
    let reporter = authorizer.clone();
    tokio::spawn(async move {
        let _ = reporter.report_authenticated(peer_key).await;
    });
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| SessionError::InvalidTime)?
        .as_millis();
    stream
        .send_json(
            &DeviceSessionAuthenticationResult::Accepted {
                user_context: authorized.user_context.clone(),
                device_ref,
                operator: authorized.operator,
                password_version: credential.version(),
                authenticated_at_unix_ms: i64::try_from(now)
                    .map_err(|_| SessionError::InvalidTime)?,
            },
            timeout,
        )
        .await?;
    Ok(authorized)
}

async fn reject(
    stream: &mut pab_transport::PabBiStream,
    timeout: Duration,
) -> Result<(), PabConnectionError> {
    stream
        .send_json(&DeviceSessionAuthenticationResult::Rejected, timeout)
        .await
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("device session authentication was rejected")]
    Rejected,
    #[error("device session identity mismatch")]
    IdentityMismatch,
    #[error("invalid device password hash or version")]
    InvalidCredential,
    #[error("password verification worker failed")]
    PasswordWorker,
    #[error("invalid session timeout")]
    InvalidTimeout,
    #[error("invalid system time")]
    InvalidTime,
    #[error(transparent)]
    Authorization(#[from] PeerAuthorizationError),
    #[error(transparent)]
    Connection(#[from] PabConnectionError),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presence_counts_operators_and_releases_each_connection() {
        let sessions = ActiveSessions::default();
        let first = sessions.enter(EndpointKey::new([1; 32]));
        let duplicate = sessions.enter(EndpointKey::new([1; 32]));
        let second = sessions.enter(EndpointKey::new([2; 32]));
        assert_eq!(sessions.operator_count(), 2);
        drop(first);
        assert_eq!(sessions.operator_count(), 2);
        drop(duplicate);
        assert_eq!(sessions.operator_count(), 1);
        drop(second);
        assert_eq!(sessions.operator_count(), 0);
    }
}
