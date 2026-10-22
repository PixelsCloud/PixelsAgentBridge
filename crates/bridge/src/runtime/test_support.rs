use super::*;
use pab_protocol::RequestId;
use std::{path::Path, sync::atomic::AtomicU64};
use tokio::sync::{Notify, broadcast};

pub(super) async fn runtime(path: &Path) -> Arc<BridgeRuntime> {
    let store = RuntimeStore::open(&path.join("runtime.sqlite3"))
        .await
        .unwrap();
    store.start_session("fixture").await.unwrap();
    let (shutdown, receiver) = watch::channel(false);
    let account_store = pab_agent_core::account::AccountStore::new(
        path,
        &format!("https://{}.invalid", RequestId::new()),
    )
    .unwrap();
    let user = pab_agent_core::account::AccountUser {
        id: pab_protocol::UserId::new(),
        username: "fixture".into(),
        server_admin: false,
    };
    let account = account_store
        .login(pab_agent_core::account::AccountSession {
            user: user.clone(),
            access_token: "fixture-token".into(),
        })
        .unwrap();
    Arc::new(BridgeRuntime {
        account_task: tokio::spawn(async {}),
        usage_task: tokio::spawn(async {}),
        inner: Arc::new(RuntimeInner {
            account_store,
            presence: std::sync::Mutex::new(crate::desktop_presence::RuntimeReport {
                account: Some(crate::desktop_presence::AccountSyncReport {
                    local_revision: account.revision,
                    server_revision: Some(account.revision),
                    user: Some(pab_protocol::UserAttribution {
                        user_id: user.id,
                        username: user.username,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            store,
            screenshot_dir: path.join("screenshots"),
            terminal_dir: path.to_owned(),
            session_id: "fixture".into(),
            passwords: Arc::new(MemoryDevicePasswordProvider::default()),
            retry_interval: Duration::from_secs(1),
            events: broadcast::channel(10).0,
            event_sequence: AtomicU64::new(0),
            availability: watch::channel(BridgeAvailability::Connecting).1,
            shutdown: receiver,
            devices: Mutex::new(HashMap::new()),
            device_codes: Mutex::new(HashMap::new()),
            initiated_by: "fixture".into(),
            operations: Mutex::new(HashMap::new()),
            terminals: Mutex::new(HashMap::new()),
            port_forwards: Arc::new(crate::runtime::port_forward::PortForwardManager::default()),
            reconciliation_notify: Notify::new(),
        }),
        shutdown,
        bridge_task: tokio::spawn(async {}),
        heartbeat_task: tokio::spawn(async {}),
        reconciliation_task: tokio::spawn(async {}),
    })
}
