use base64::{Engine, engine::general_purpose::STANDARD};
use pab_protocol::{MAX_TERMINAL_INPUT_BYTES, RequestId};
use serde::Serialize;
use tauri::State;

use super::{OperatorState, parse_code};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOpenedView {
    session_id: RequestId,
    shell: String,
    cols: u16,
    rows: u16,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOutputView {
    data: String,
    ended: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalEventView {
    sequence: u64,
    kind: String,
    data: String,
    state: String,
    at_unix_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalHistoryView {
    output: String,
    events: Vec<TerminalEventView>,
}

#[tauri::command]
pub async fn operator_open_terminal(
    state: State<'_, OperatorState>,
    code: String,
    cols: u16,
    rows: u16,
) -> Result<TerminalOpenedView, String> {
    let runtime = state.runtime().await?;
    let device_ref = runtime
        .resolve_device_code(parse_code(&code)?)
        .await
        .map_err(|error| error.to_string())?;
    let opened = runtime
        .open_terminal(device_ref, cols, rows)
        .await
        .map_err(|error| error.to_string())?;
    state
        .terminal_runtimes
        .lock()
        .await
        .insert(opened.session_id, runtime);
    Ok(TerminalOpenedView {
        session_id: opened.session_id,
        shell: opened.shell,
        cols: opened.cols,
        rows: opened.rows,
    })
}

#[tauri::command]
pub async fn operator_terminal_input(
    state: State<'_, OperatorState>,
    id: String,
    data: String,
) -> Result<(), String> {
    let id = parse_id(&id)?;
    let bytes = STANDARD.decode(data).map_err(|error| error.to_string())?;
    if bytes.is_empty() || bytes.len() > MAX_TERMINAL_INPUT_BYTES {
        return Err("terminal input must be between 1 and 4096 bytes".to_owned());
    }
    terminal_runtime(&state, id)
        .await?
        .terminal_input(id, &bytes)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn operator_terminal_read(
    state: State<'_, OperatorState>,
    id: String,
) -> Result<TerminalOutputView, String> {
    let id = parse_id(&id)?;
    let output = terminal_runtime(&state, id)
        .await?
        .terminal_read(id)
        .await
        .map_err(|error| error.to_string())?;
    Ok(TerminalOutputView {
        data: STANDARD.encode(output.bytes),
        ended: output.ended,
    })
}

#[tauri::command]
pub async fn operator_terminal_resize(
    state: State<'_, OperatorState>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let id = parse_id(&id)?;
    terminal_runtime(&state, id)
        .await?
        .terminal_resize(id, cols, rows)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn operator_terminal_close(
    state: State<'_, OperatorState>,
    id: String,
) -> Result<(), String> {
    let id = parse_id(&id)?;
    let runtime = terminal_runtime(&state, id).await?;
    runtime
        .terminal_close(id)
        .await
        .map_err(|error| error.to_string())?;
    state.terminal_runtimes.lock().await.remove(&id);
    Ok(())
}

#[tauri::command]
pub async fn operator_terminal_history(
    state: State<'_, OperatorState>,
    id: String,
) -> Result<Option<TerminalHistoryView>, String> {
    parse_id(&id)?;
    let local = state.local_store().await?;
    let Some(output) = local
        .terminal_bytes(&id)
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let events = local
        .terminal_events(&id)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|event| TerminalEventView {
            sequence: event.sequence,
            kind: event.kind,
            data: STANDARD.encode(event.payload),
            state: event.state,
            at_unix_ms: event.at_unix_ms,
        })
        .collect();
    Ok(Some(TerminalHistoryView {
        output: STANDARD.encode(output),
        events,
    }))
}

fn parse_id(value: &str) -> Result<RequestId, String> {
    value
        .parse()
        .map_err(|_| "invalid terminal session ID".to_owned())
}

async fn terminal_runtime(
    state: &State<'_, OperatorState>,
    id: RequestId,
) -> Result<std::sync::Arc<pab_bridge::BridgeRuntime>, String> {
    state
        .terminal_runtimes
        .lock()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(|| "terminal is not in this desktop session".to_owned())
}
