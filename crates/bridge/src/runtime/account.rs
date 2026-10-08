use super::{RuntimeError, RuntimeInner};
use crate::{BridgeConfig, desktop_presence::AccountSyncReport};
use pab_agent_core::account::{AccountClient, AccountError, AccountStore};
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

pub(super) async fn run(
    config: BridgeConfig,
    inner: Arc<RuntimeInner>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut notifications = pab_agent_core::account::account_changes();
    let mut interval = tokio::time::interval(Duration::from_secs(3));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = interval.tick() => {},
            _ = notifications.changed() => {},
        }
        let result = tokio::select! {
            _ = shutdown.changed() => break,
            result = synchronize(&config, &inner) => result,
        };
        if let Err(error) = result {
            if let Some(account) = inner
                .presence
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .account
                .as_mut()
            {
                account.error = Some(error.to_string());
            }
        }
    }
}

async fn synchronize(config: &BridgeConfig, inner: &Arc<RuntimeInner>) -> Result<(), AccountError> {
    let paths = pab_agent_core::DataPaths::for_scope(pab_agent_core::DataScope::User)
        .map_err(|_| AccountError::Storage)?;
    let store = AccountStore::new(paths.root(), &config.control_url)?;
    let state = store.read()?;
    {
        let mut presence = inner.presence.lock().unwrap_or_else(|e| e.into_inner());
        let status = presence
            .account
            .get_or_insert_with(AccountSyncReport::default);
        if status.local_revision != state.revision {
            *status = AccountSyncReport {
                local_revision: state.revision,
                ..Default::default()
            };
        }
    }
    let ca = config
        .control_ca_cert
        .as_ref()
        .map(std::fs::read)
        .transpose()
        .map_err(|_| AccountError::Storage)?;
    let client = AccountClient::new(&config.control_url, ca.as_deref())?;
    let secret = pab_agent_core::read_endpoint_secret(&config.endpoint_secret_file)
        .map_err(|_| AccountError::Storage)?;
    let token = state
        .active_slot
        .as_ref()
        .map(|slot| store.token(slot))
        .transpose()?
        .flatten();
    if state.user.is_some() && token.is_none() {
        return Err(AccountError::Storage);
    }
    let result = client
        .bind_endpoint(
            &secret,
            state.revision,
            token.as_ref().map(|value| value.as_str()),
        )
        .await;
    let (receipt, unavailable) = match result {
        Ok(receipt) => {
            let unavailable = state.user.is_some() && receipt.context.user.is_none();
            (receipt, unavailable)
        }
        Err(AccountError::Http(401)) => (
            client.bind_endpoint(&secret, state.revision, None).await?,
            true,
        ),
        Err(error) => return Err(error),
    };
    let context = receipt.context;
    // An in-flight login response may never overwrite a newer local account.
    if store.read()?.revision != state.revision {
        return Ok(());
    }
    sqlx::query("INSERT INTO runtime_session_users(session_id,user_json) VALUES(?,?) ON CONFLICT(session_id) DO UPDATE SET user_json=excluded.user_json")
        .bind(&inner.session_id).bind(serde_json::to_string(&context.user).map_err(|_|AccountError::InvalidResponse)?).execute(&inner.store.pool).await.map_err(|_|AccountError::Storage)?;
    {
        let mut presence = inner.presence.lock().unwrap_or_else(|e| e.into_inner());
        presence.account = Some(AccountSyncReport {
            relay_nodes: receipt.relay_nodes,
            local_revision: state.revision,
            server_revision: Some(context.revision),
            remote_revision: None,
            policy_version: Some(context.policy_version),
            user: context.user.clone(),
            error: unavailable
                .then(|| "account unavailable; device control uses guest privileges".to_owned()),
        });
    }
    let devices = inner
        .devices
        .lock()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let mut checks = tokio::task::JoinSet::new();
    for device in devices {
        if let Some(connection) = device.cached_connection().await {
            let expected = context.clone();
            checks.spawn(async move {
                matches!(tokio::time::timeout(Duration::from_secs(5), connection.user_context()).await,
                    Ok(Ok(remote)) if remote.revision == expected.revision && remote.user == expected.user)
            });
        }
    }
    let mut all_confirmed = !checks.is_empty();
    while let Some(result) = checks.join_next().await {
        all_confirmed &= result.unwrap_or(false);
    }
    if store.read()?.revision == state.revision && all_confirmed {
        if let Some(status) = inner
            .presence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .account
            .as_mut()
        {
            status.remote_revision = Some(context.revision);
        }
    }
    let _ = client.flush_logouts(&store).await;
    Ok(())
}

impl RuntimeInner {
    pub(super) async fn wait_account_ready(&self) -> Result<(), RuntimeError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let status = self
                .presence
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .account
                .clone();
            let Some(status) = status else {
                return Ok(());
            };
            let origin = self
                .presence
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .control_url
                .clone();
            let paths = pab_agent_core::DataPaths::for_scope(pab_agent_core::DataScope::User)
                .map_err(|_| {
                    RuntimeError::BridgeUnavailable("account storage unavailable".into())
                })?;
            let latest = AccountStore::new(paths.root(), &origin)
                .and_then(|store| store.read())
                .map_err(|_| RuntimeError::BridgeUnavailable("account storage unavailable".into()))?
                .revision;
            if status.server_revision == Some(latest) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(RuntimeError::BridgeUnavailable(
                    "account synchronization is pending".into(),
                ));
            }
            self.wait_or_shutdown(Duration::from_millis(100)).await?;
        }
    }
}
