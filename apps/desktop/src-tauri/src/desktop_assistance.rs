use pab_bridge::desktop_assistance::{Snapshot, Status, Store};
use pab_protocol::DeviceCode;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tauri::{Manager, State, WebviewWindow};
use tokio::sync::{Mutex, watch};
use uuid::Uuid;

struct Inner {
    instance: Uuid,
    store: Mutex<Option<Store>>,
    bindings: Mutex<HashMap<Uuid, String>>,
    notified: Mutex<HashSet<Uuid>>,
    stopped: watch::Sender<bool>,
}
#[derive(Clone)]
pub(crate) struct AssistanceState(Arc<Inner>);
impl Default for AssistanceState {
    fn default() -> Self {
        Self(Arc::new(Inner {
            instance: Uuid::new_v4(),
            store: Mutex::new(None),
            bindings: Mutex::new(HashMap::new()),
            notified: Mutex::new(HashSet::new()),
            stopped: watch::channel(false).0,
        }))
    }
}
impl AssistanceState {
    pub(crate) async fn device_removed(&self, code: DeviceCode) -> Result<(), String> {
        if pab_agent_core::account::AccountStore::from_env()
            .and_then(|store| store.read())
            .map_err(|e| e.to_string())?
            .user
            .is_none()
        {
            return Ok(());
        }
        self.store()
            .await?
            .device_removed(self.0.instance, code)
            .await
            .map_err(|e| e.to_string())
    }
    async fn store(&self) -> Result<Store, String> {
        let mut slot = self.0.store.lock().await;
        if slot.as_ref().is_some_and(|s| !s.is_current_account()) {
            let old = slot.take().unwrap();
            let _ = old.stop_desktop(self.0.instance).await;
            self.0.bindings.lock().await.clear();
            self.0.notified.lock().await.clear();
        }
        if slot.is_none() {
            let store = Store::for_current_account()
                .await
                .map_err(|e| e.to_string())?;
            store
                .heartbeat_desktop(self.0.instance)
                .await
                .map_err(|e| e.to_string())?;
            *slot = Some(store);
        }
        let store = slot.as_ref().unwrap().clone();
        Ok(store)
    }
    pub(crate) fn stop(&self) {
        self.0.stopped.send_replace(true);
    }
    async fn pending(&self) -> Result<(Store, Vec<Snapshot>), String> {
        let store = self.store().await?;
        let pending = store
            .pending(self.0.instance)
            .await
            .map_err(|e| e.to_string())?;
        if !store.is_current_account() {
            return Err("account changed".into());
        }
        Ok((store, pending))
    }
}
pub(crate) fn start(app: tauri::AppHandle, state: AssistanceState) {
    tauri::async_runtime::spawn(async move {
        let mut stopped = state.0.stopped.subscribe();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut heartbeat = tokio::time::Instant::now();
        loop {
            tokio::select! {_=stopped.changed()=>break,_=tick.tick()=>{}}
            if tokio::time::Instant::now() >= heartbeat {
                if let Ok(store) = state.store().await {
                    let _ = store.heartbeat_desktop(state.0.instance).await;
                }
                heartbeat = tokio::time::Instant::now() + pab_bridge::desktop_assistance::HEARTBEAT;
            }
            let Ok((store, pending)) = state.pending().await else {
                continue;
            };
            let ids: HashSet<_> = pending.iter().map(|s| s.assistance_id).collect();
            let mut bindings = state.0.bindings.lock().await;
            bindings.retain(|id, _| ids.contains(id));
            for entry in &pending {
                if let Some(label) = bindings.get(&entry.assistance_id) {
                    if app
                        .state::<crate::operator::remote_desktop::RemoteDesktopState>()
                        .assistance_viewing(label, entry.request.device_code)
                        .await
                    {
                        let _ = store
                            .activate(state.0.instance, entry.assistance_id, label)
                            .await;
                    } else if entry.status == Status::Active {
                        let _ = store.window_closed(state.0.instance, label).await;
                    }
                }
            }
            drop(bindings);
            let mut notified = state.0.notified.lock().await;
            notified.retain(|id| ids.contains(id));
            if pending.iter().any(|s| !notified.contains(&s.assistance_id)) {
                // This only brings the prompt's own operator to the foreground.
                // Opening a viewer and enabling input remain explicit UI actions.
                crate::tray::show_window(&app);
                notified.extend(ids);
            }
        }
        if let Some(store) = state.0.store.lock().await.take() {
            let _ = store.stop_desktop(state.0.instance).await;
        }
    });
}
#[tauri::command]
pub(crate) async fn desktop_assistance_pending(
    window: WebviewWindow,
    state: State<'_, AssistanceState>,
) -> Result<Vec<Snapshot>, String> {
    if window.label() != "main" {
        return Err("only main may list assistance prompts".into());
    }
    // A signed-out Desktop simply has no pending prompts.
    if pab_agent_core::account::AccountStore::from_env()
        .and_then(|s| s.read())
        .ok()
        .and_then(|s| s.user)
        .is_none()
    {
        return Ok(Vec::new());
    }
    state.pending().await.map(|(_, rows)| rows)
}
#[tauri::command]
pub(crate) async fn desktop_assistance_displayed(
    window: WebviewWindow,
    state: State<'_, AssistanceState>,
    id: Uuid,
) -> Result<bool, String> {
    if window.label() != "main" {
        return Err("only main may acknowledge a prompt".into());
    }
    let store = state.store().await?;
    // The WebView can finish its modal animation while its native window is
    // hidden or minimized. Leave delivery pending until the prompt is visible.
    if !window.is_visible().map_err(|e| e.to_string())?
        || window.is_minimized().map_err(|e| e.to_string())?
    {
        return Ok(false);
    }
    store
        .displayed(state.0.instance, id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(true)
}
#[tauri::command]
pub(crate) async fn desktop_assistance_open(
    app: tauri::AppHandle,
    window: WebviewWindow,
    state: State<'_, AssistanceState>,
    id: Uuid,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("only main may open assistance".into());
    }
    let operator = app.state::<crate::operator::OperatorState>();
    let _guard = operator.account_changes.lock().await;
    let (store, pending) = state.pending().await?;
    let request = pending
        .into_iter()
        .find(|s| s.assistance_id == id)
        .ok_or("assistance request has ended")?;
    let label = crate::operator::remote_desktop::open_window(
        app.clone(),
        &operator,
        &app.state::<crate::operator::remote_desktop::RemoteDesktopState>(),
        request.request.device_code.to_string(),
        request.request.screen_id,
        true,
    )
    .await?;
    if !store.is_current_account() {
        return Err("account changed".into());
    }
    // Binding is not activation: only the native viewer's viewing status can
    // activate this request in the lifecycle observer above.
    state.0.bindings.lock().await.insert(id, label);
    Ok(())
}
#[tauri::command]
pub(crate) async fn desktop_assistance_cancel(
    window: WebviewWindow,
    state: State<'_, AssistanceState>,
    id: Uuid,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("only main may dismiss assistance".into());
    }
    let store = state.store().await?;
    store
        .dismiss(state.0.instance, id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}
#[tauri::command]
pub(crate) async fn desktop_viewer_assistance(
    window: WebviewWindow,
    state: State<'_, AssistanceState>,
) -> Result<Vec<Snapshot>, String> {
    if !window.label().starts_with("desktop-viewer-") {
        return Err("remote desktop window required".into());
    }
    let (_, pending) = state.pending().await?;
    Ok(pending
        .into_iter()
        .filter(|s| s.window_label.as_deref() == Some(window.label()))
        .collect())
}
#[tauri::command]
pub(crate) async fn desktop_viewer_assistance_complete(
    app: tauri::AppHandle,
    window: WebviewWindow,
    state: State<'_, AssistanceState>,
    id: Uuid,
) -> Result<(), String> {
    let (store, pending) = state.pending().await?;
    let entry = pending
        .into_iter()
        .find(|s| s.assistance_id == id && s.window_label.as_deref() == Some(window.label()))
        .ok_or("assistance is not active in this window")?;
    if !app
        .state::<crate::operator::remote_desktop::RemoteDesktopState>()
        .assistance_viewing(window.label(), entry.request.device_code)
        .await
    {
        return Err("remote desktop is not connected".into());
    }
    store
        .complete(state.0.instance, id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}
