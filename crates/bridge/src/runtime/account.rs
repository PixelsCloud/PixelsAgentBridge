use super::{RuntimeError, RuntimeInner};
use crate::{BridgeConfig, desktop_presence::AccountSyncReport};
use pab_agent_core::account::{AccountClient, AccountError};
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
    let mut next_association = tokio::time::Instant::now();
    let mut association: Option<tokio::task::JoinHandle<()>> = None;
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = interval.tick() => {},
            _ = notifications.changed() => {next_association=tokio::time::Instant::now();},
        }
        if tokio::time::Instant::now() >= next_association
            && association.as_ref().is_none_or(|task| task.is_finished())
        {
            next_association = tokio::time::Instant::now() + Duration::from_secs(30);
            association = Some(tokio::spawn(async {
                let _ = pab_agent_core::account::local_device::associate(
                    pab_protocol::DeviceAccountAction::Automatic,
                    None,
                )
                .await;
            }));
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
    if let Some(task) = association {
        task.abort();
    }
}

async fn synchronize(config: &BridgeConfig, inner: &Arc<RuntimeInner>) -> Result<(), AccountError> {
    let store = &inner.account_store;
    let state = store.read()?;
    // Close locally as soon as logout is observed, even if the server is unreachable.
    if state.user.is_none() {
        close_device_connections(inner).await;
    }
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
        Err(AccountError::Http(401)) => {
            close_device_connections(inner).await;
            if let Some(status) = inner
                .presence
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .account
                .as_mut()
            {
                status.user = None;
                status.server_revision = Some(state.revision);
            }
            return Err(AccountError::Http(401));
        }
        Err(error) => return Err(error),
    };
    let context = receipt.context;
    // An in-flight login response may never overwrite a newer local account.
    if store.read()?.revision != state.revision {
        return Ok(());
    }
    if context.user.is_none() {
        close_device_connections(inner).await;
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
            error: unavailable.then(|| {
                "account unavailable; sign in again before controlling devices".to_owned()
            }),
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
        let scope = context
            .user
            .as_ref()
            .map(|user| format!("{}\n{}", client.origin(), user.user_id));
        device.sample_usage(scope.as_deref()).await;
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
    let catalog = inner.store.clone();
    let store = inner.account_store.clone();
    tokio::spawn(async move {
        let _ = catalog.sync_catalog(&client, &store).await;
    });
    Ok(())
}

async fn close_device_connections(inner: &RuntimeInner) {
    let devices = inner
        .devices
        .lock()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for device in devices {
        device.close_for_account_change().await;
    }
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
            let latest = self.account_store.read().map_err(|_| {
                RuntimeError::BridgeUnavailable("account storage unavailable".into())
            })?;
            if latest.user.is_none() {
                return Err(RuntimeError::BridgeUnavailable(
                    "login required: register or sign in to Pixels Agent Bridge before adding or controlling devices".into(),
                ));
            }
            if let Some(status) = status {
                if status.server_revision == Some(latest.revision) {
                    if status.user.as_ref().map(|user| user.user_id)
                        == latest.user.as_ref().map(|user| user.id)
                        && status.user.is_some()
                    {
                        return Ok(());
                    }
                    return Err(RuntimeError::BridgeUnavailable(
                        "account unavailable; sign in again before controlling devices".into(),
                    ));
                }
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
