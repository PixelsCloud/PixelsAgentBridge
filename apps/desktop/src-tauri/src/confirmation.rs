//! App-styled confirmations in an owned WebView, above native video surfaces.
//! Only the generated dialog can answer its own one-shot request.
use serde::Serialize;
use std::{collections::HashMap, sync::Mutex, time::Duration};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::oneshot;

#[derive(Clone, Serialize)]
pub struct Question { title: String, body: String, confirm: String, cancel: String }
struct Pending { question: Question, answer: Option<oneshot::Sender<bool>>, ready: bool, parent: Option<String> }
#[derive(Default)]
pub struct ConfirmationState(Mutex<HashMap<String, Pending>>);

fn answer(state: &ConfirmationState, label: &str, accepted: bool) -> Result<(), String> {
    let mut requests = state.0.lock().unwrap();
    let pending = requests.get_mut(label).ok_or("Confirmation is closed")?;
    let sender = pending.answer.take().ok_or("Confirmation is closed")?;
    let _ = sender.send(accepted && pending.ready);
    Ok(())
}

#[tauri::command]
pub fn confirmation_info(window: WebviewWindow, state: State<'_, ConfirmationState>) -> Result<Question, String> {
    state.0.lock().unwrap().get(window.label()).map(|p| p.question.clone()).ok_or("Confirmation is closed".into())
}

#[tauri::command]
pub fn confirmation_ready(window: WebviewWindow, state: State<'_, ConfirmationState>, height: f64) -> Result<(), String> {
    if !height.is_finite() { return Err("Invalid dialog size".into()); }
    let mut pending = state.0.lock().unwrap();
    let request = pending.get_mut(window.label()).ok_or("Confirmation is closed")?;
    if request.ready { return Ok(()); }
    window.set_size(tauri::LogicalSize::new(628.0, height.clamp(160.0, 560.0))).map_err(|e| e.to_string())?;
    window.center().map_err(|e| e.to_string())?;
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    request.ready = true;
    Ok(())
}

#[tauri::command]
pub fn confirmation_answer(window: WebviewWindow, state: State<'_, ConfirmationState>, accepted: bool) -> Result<(), String> {
    answer(&state, window.label(), accepted)
}

struct Cleanup { app: AppHandle, label: String, parent: Option<WebviewWindow> }
impl Drop for Cleanup {
    fn drop(&mut self) {
        self.app.state::<ConfirmationState>().0.lock().unwrap().remove(&self.label);
        if let Some(parent) = &self.parent { let _ = parent.set_enabled(true); }
        if let Some(dialog) = self.app.get_webview_window(&self.label) { let _ = dialog.destroy(); }
        if let Some(parent) = &self.parent { let _ = parent.set_focus(); }
    }
}

pub async fn ask(app: &AppHandle, parent: Option<&WebviewWindow>, title: &str, body: &str, confirm: &str, cancel: &str) -> bool {
    let result = ask_inner(app, parent, Question { title: title.into(), body: body.into(), confirm: confirm.into(), cancel: cancel.into() }).await;
    if let Err(error) = &result { tracing::warn!(%error, "application confirmation cancelled"); }
    result.unwrap_or(false)
}

async fn ask_inner(app: &AppHandle, parent: Option<&WebviewWindow>, question: Question) -> Result<bool, String> {
    let label = format!("confirmation-{}", pab_protocol::RequestId::new());
    let (sender, mut receiver) = oneshot::channel();
    let parent_label = parent.map(|p| p.label().to_owned());
    {
        let state = app.state::<ConfirmationState>();
        let mut pending = state.0.lock().unwrap();
        // Never re-enable an owner while a different confirmation still owns it.
        if pending.values().any(|p| p.parent == parent_label) { return Ok(false); }
        pending.insert(label.clone(), Pending { question: question.clone(), answer: Some(sender), ready: false, parent: parent_label });
    }
    let _cleanup = Cleanup { app: app.clone(), label: label.clone(), parent: parent.cloned() };
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html?confirmation=1".into()))
        .title(&question.title).inner_size(628.0, 280.0).decorations(false).resizable(false)
        .minimizable(false).maximizable(false).skip_taskbar(true).visible(false);
    let builder = if let Some(parent) = parent { builder.parent(parent).map_err(|e| e.to_string())? } else { builder };
    let dialog = builder.build().map_err(|e| e.to_string())?;
    let handle = app.clone(); let event_label = label.clone();
    dialog.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed) {
            let _ = answer(&handle.state::<ConfirmationState>(), &event_label, false);
        }
    });
    if let Some(parent) = parent { parent.set_enabled(false).map_err(|e| e.to_string())?; }
    // A failed WebView load must never leave the owner disabled indefinitely.
    tokio::select! {
        result = &mut receiver => return Ok(result.unwrap_or(false)),
        _ = tokio::time::sleep(Duration::from_secs(15)) => {},
    }
    if !app.state::<ConfirmationState>().0.lock().unwrap().get(&label).is_some_and(|p| p.ready) { return Ok(false); }
    // Also cancel if the owner disappears on platforms that do not destroy children.
    loop {
        tokio::select! {
            result = &mut receiver => return Ok(result.unwrap_or(false)),
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                if parent.is_some_and(|p| app.get_webview_window(p.label()).is_none()) { return Ok(false); }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn answers_are_window_bound_one_shot_and_require_ready() {
        let state = ConfirmationState::default();
        for (ready, accepted) in [(true, true), (true, false), (false, true)] {
            let (tx, mut rx) = oneshot::channel();
            state.0.lock().unwrap().insert("confirmation-a".into(), Pending { question: Question { title: "".into(), body: "".into(), confirm: "".into(), cancel: "".into() }, answer: Some(tx), ready, parent: None });
            assert!(answer(&state, "main", true).is_err());
            answer(&state, "confirmation-a", accepted).unwrap();
            assert_eq!(rx.try_recv().unwrap(), ready && accepted);
            assert!(answer(&state, "confirmation-a", true).is_err());
        }
    }
}
