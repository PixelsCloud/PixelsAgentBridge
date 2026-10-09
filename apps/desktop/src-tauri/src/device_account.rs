use pab_agent_core::account::{AccountStore, local_device};
use pab_protocol::{DeviceAccountAction, DeviceAccountState};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

#[derive(Clone, Default, Serialize)]
pub struct Status {
    pub account_revision: u64,
    pub device: Option<DeviceAccountState>,
    pub pending: bool,
}

#[derive(Clone, Default)]
pub struct DeviceAccountService {
    state: Arc<RwLock<Status>>,
    operation: Arc<Mutex<()>>,
}

impl DeviceAccountService {
    async fn update(
        &self,
        action: DeviceAccountAction,
        revision: Option<i64>,
        expected_account: Option<u64>,
    ) -> Result<Status, String> {
        let _guard = self.operation.lock().await;
        let store = AccountStore::from_env().map_err(|e| e.to_string())?;
        let account = store.read().map_err(|e| e.to_string())?;
        if expected_account.is_some_and(|revision| revision != account.revision) {
            return Err("account_changed".into());
        }
        let result = local_device::associate_at(action, revision, Some(account.revision)).await;
        if store.read().map_err(|e| e.to_string())?.revision != account.revision {
            return Err("account_changed".into());
        }
        let mut state = self.state.write().await;
        state.account_revision = account.revision;
        match result {
            Ok(device) => {
                state.device = device;
                state.pending = false;
                Ok(state.clone())
            }
            Err(error) => {
                state.device = None;
                state.pending = account.user.is_some();
                Err(error.to_string())
            }
        }
    }
    pub async fn run(self) {
        let mut changes = pab_agent_core::account::account_changes();
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {_=interval.tick()=>{}, r=changes.changed()=>{if r.is_err(){break;}}}
            if self
                .update(DeviceAccountAction::Automatic, None, None)
                .await
                .is_err()
            {
                tracing::debug!("local device account sync pending");
            }
        }
    }
}

async fn local_store() -> Result<pab_bridge::BridgeLocalStore, String> {
    let paths = pab_agent_core::DataPaths::for_scope(pab_agent_core::DataScope::User)
        .map_err(|e| e.to_string())?;
    pab_bridge::BridgeLocalStore::open(&paths.bridge_database())
        .await
        .map_err(|e| e.to_string())
}
pub async fn run_catalog() {
    let mut changes = pab_agent_core::account::account_changes();
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {_=interval.tick()=>{},r=changes.changed()=>{if r.is_err(){return;}}}
        if let Ok(local) = local_store().await {
            let _ = local.sync_account_catalog().await;
        }
    }
}
#[derive(Serialize)]
pub struct CatalogView {
    account_revision: u64,
    #[serde(flatten)]
    status: pab_bridge::CatalogStatus,
}
#[tauri::command]
pub async fn account_catalog_status() -> Result<CatalogView, String> {
    let account = AccountStore::from_env()
        .and_then(|s| s.read())
        .map_err(|e| e.to_string())?;
    let status = local_store()
        .await?
        .catalog_status()
        .await
        .map_err(|e| e.to_string())?;
    if AccountStore::from_env()
        .and_then(|s| s.read())
        .map_err(|e| e.to_string())?
        .revision
        != account.revision
    {
        return Err("account_changed".into());
    }
    Ok(CatalogView {
        account_revision: account.revision,
        status,
    })
}
#[tauri::command]
pub async fn account_catalog_action(
    revision: u64,
    action: String,
    device: Option<pab_protocol::DeviceId>,
) -> Result<(), String> {
    let account = AccountStore::from_env()
        .and_then(|s| s.read())
        .map_err(|e| e.to_string())?;
    if account.revision != revision || account.user.is_none() {
        return Err("account_changed".into());
    }
    let local = local_store().await?;
    match action.as_str() {
        "import" => {
            local
                .import_local_devices_at(revision)
                .await
                .map_err(|e| e.to_string())?;
        }
        "keep_local" | "keep_remote" => {
            local
                .resolve_catalog_conflict_at(
                    device.ok_or("device_required")?,
                    action == "keep_local",
                    revision,
                )
                .await
                .map_err(|e| e.to_string())?;
        }
        "retry" => {}
        _ => return Err("invalid_action".into()),
    }
    tokio::spawn(async move {
        let _ = local.sync_account_catalog().await;
    });
    Ok(())
}

#[tauri::command]
pub async fn device_account_status(
    service: tauri::State<'_, DeviceAccountService>,
) -> Result<Status, String> {
    let account = AccountStore::from_env()
        .and_then(|s| s.read())
        .map_err(|e| e.to_string())?;
    let status = service.state.read().await.clone();
    if status.account_revision == account.revision {
        Ok(status)
    } else {
        Ok(Status {
            account_revision: account.revision,
            device: None,
            pending: account.user.is_some(),
        })
    }
}

#[tauri::command]
pub async fn device_account_associate(
    service: tauri::State<'_, DeviceAccountService>,
    action: DeviceAccountAction,
    revision: i64,
    account_revision: u64,
) -> Result<Status, String> {
    service
        .update(action, Some(revision), Some(account_revision))
        .await
}
