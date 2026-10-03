use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool},
};

use pab_agent_core::{
    DataPaths, DataScope, login_traffic_scopes, read_endpoint_secret,
    register_account_traffic_scope, tls_connector,
};
use pab_bridge::{
    BridgeConfig, BridgeLocalStore, BridgeRuntime, BridgeRuntimeConfig,
    ConnectionPath, MemoryDevicePasswordProvider, RememberedDevice, SqliteDevicePasswordProvider,
};
use pab_protocol::{DeviceCode, EndpointKey, OutputStream, RequestId, TaskId, TaskRef};
use serde::Serialize;
use tauri::Emitter;
use tokio::sync::{Mutex, OnceCell};
use tokio::task::AbortHandle;
use zeroize::Zeroizing;

pub(crate) mod directory;
pub(crate) mod history;
pub(crate) mod screenshot;
pub(crate) mod terminal;
pub(crate) mod windows;

pub struct OperatorState {
    runtimes: Mutex<RuntimeSelection>,
    passwords: Arc<MemoryDevicePasswordProvider>,
    tasks: Mutex<HashMap<TaskId, (TaskRef, Arc<BridgeRuntime>)>>,
    terminal_runtimes: Mutex<HashMap<RequestId, Arc<BridgeRuntime>>>,
    transfers: Arc<Mutex<HashMap<String, AbortHandle>>>,
    events_started: AtomicBool,
    local: OnceCell<Arc<BridgeLocalStore>>,
}

struct RuntimeSelection {
    active: String,
    active_scope: Option<ScopeStatus>,
    runtimes: HashMap<String, Arc<BridgeRuntime>>,
    endpoints: HashMap<EndpointKey, Arc<BridgeRuntime>>,
}

impl OperatorState {
    pub fn new() -> Self {
        let credential_database = DataPaths::for_scope(DataScope::User)
            .expect("could not find user credential database path")
            .bridge_database();
        Self {
            runtimes: Mutex::new(RuntimeSelection {
                active: "guest".to_owned(),
                active_scope: None,
                runtimes: HashMap::new(),
                endpoints: HashMap::new(),
            }),
            passwords: Arc::new(MemoryDevicePasswordProvider::new(Some(Box::new(
                SqliteDevicePasswordProvider::new(credential_database),
            )))),
            tasks: Mutex::new(HashMap::new()),
            terminal_runtimes: Mutex::new(HashMap::new()),
            transfers: Arc::new(Mutex::new(HashMap::new())),
            events_started: AtomicBool::new(false),
            local: OnceCell::new(),
        }
    }

    async fn runtime(&self) -> Result<Arc<BridgeRuntime>, String> {
        let mut selection = self.runtimes.lock().await;
        if let Some(runtime) = selection.runtimes.get(&selection.active) {
            return Ok(Arc::clone(runtime));
        }
        let config = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            BridgeConfig::register_guest_from_env(),
        )
        .await
        .map_err(|_| "guest registration timed out".to_owned())?
        .map_err(|error| error.to_string())?;
        let secret = read_endpoint_secret(&config.endpoint_secret_file)
            .map_err(|error| error.to_string())?;
        let endpoint_key = EndpointKey::new(*secret.public().as_bytes());
        let paths = DataPaths::for_scope(DataScope::User).map_err(|error| error.to_string())?;
        let mut runtime_config = BridgeRuntimeConfig::new(paths.bridge_database());
        runtime_config.resume_incomplete_on_start = false;
        let runtime = BridgeRuntime::start(config, runtime_config, self.passwords.clone())
            .await
            .map_err(|error| error.to_string())?;
        let runtime = Arc::new(runtime);
        selection
            .endpoints
            .insert(endpoint_key, Arc::clone(&runtime));
        selection
            .runtimes
            .insert("guest".to_owned(), Arc::clone(&runtime));
        Ok(runtime)
    }

    async fn local_store(&self) -> Result<Arc<BridgeLocalStore>, String> {
        let local = self
            .local
            .get_or_try_init(|| async {
                let paths =
                    DataPaths::for_scope(DataScope::User).map_err(|error| error.to_string())?;
                BridgeLocalStore::open(&paths.bridge_database())
                    .await
                    .map(Arc::new)
                    .map_err(|error| error.to_string())
            })
            .await?;
        Ok(Arc::clone(local))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedDevice {
    device_id: String,
    device_code: String,
    alias: String,
    os_family: String,
    os_reminder: String,
    connected: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedDevicePresence {
    device_code: String,
    name: String,
    online: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConnectionPath {
    device_code: String,
    path: Option<&'static str>,
}

#[tauri::command]
pub async fn operator_connection_paths(
    state: tauri::State<'_, OperatorState>,
) -> Result<Vec<DeviceConnectionPath>, String> {
    let remembered = state
        .local_store()
        .await?
        .remembered_devices()
        .await
        .map_err(|error| error.to_string())?;
    let runtime = state.runtime().await?;
    let mut paths = Vec::with_capacity(remembered.len());
    for device in remembered {
        let path = runtime.connection_path(device.device_ref).await.map(|path| match path {
            ConnectionPath::Direct => "p2p",
            ConnectionPath::Relay => "relay",
            ConnectionPath::Unknown => "unknown",
        });
        paths.push(DeviceConnectionPath {
            device_code: device.code.to_string(),
            path,
        });
    }
    Ok(paths)
}

#[tauri::command]
pub async fn operator_saved_device_presence(
    state: tauri::State<'_, OperatorState>,
) -> Result<Vec<SavedDevicePresence>, String> {
    let remembered = state
        .local_store()
        .await?
        .remembered_devices()
        .await
        .map_err(|error| error.to_string())?;
    let runtime = tokio::time::timeout(std::time::Duration::from_secs(5), state.runtime())
        .await
        .map_err(|_| "operator connection is unavailable".to_owned())??;
    let mut checks = tokio::task::JoinSet::new();
    for device in remembered {
        let runtime = Arc::clone(&runtime);
        checks.spawn(async move {
            let presence = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                runtime.device_presence(device.code),
            )
            .await
            .ok()
            .and_then(Result::ok);
            SavedDevicePresence {
                device_code: device.code.to_string(),
                name: presence.as_ref().map(|value| value.name.clone()).unwrap_or_default(),
                online: presence.map(|value| value.online),
            }
        });
    }
    let mut statuses = Vec::new();
    while let Some(result) = checks.join_next().await {
        statuses.push(result.map_err(|error| error.to_string())?);
    }
    Ok(statuses)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskUpdate {
    state: String,
    complete: bool,
    stdout: String,
    stderr: String,
    stdout_offset: u64,
    stderr_offset: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeStatus {
    user_id: String,
    username: String,
    tenant_id: String,
    team_name: Option<String>,
}

#[tauri::command]
pub async fn operator_current_traffic_scope(
    state: tauri::State<'_, OperatorState>,
) -> Result<Option<ScopeStatus>, String> {
    Ok(state.runtimes.lock().await.active_scope.clone())
}

#[tauri::command]
pub async fn operator_login_account(
    app: tauri::AppHandle,
    state: tauri::State<'_, OperatorState>,
    username: String,
    password: String,
) -> Result<ScopeStatus, String> {
    if username.trim().is_empty() || password.is_empty() {
        return Err("account and password are required".to_owned());
    }
    let control_url = std::env::var("PAB_CONTROL_URL")
        .map_err(|_| "PAB_CONTROL_URL is not configured".to_owned())?;
    let deployment_id = std::env::var("PAB_DEPLOYMENT_ID")
        .map_err(|_| "PAB_DEPLOYMENT_ID is not configured".to_owned())?
        .parse()
        .map_err(|_| "invalid deployment ID".to_owned())?;
    let ca = std::env::var_os("PAB_CONTROL_CA_CERT")
        .map(std::fs::read)
        .transpose()
        .map_err(|error| error.to_string())?;
    let connector = tls_connector(ca.as_deref()).map_err(|error| error.to_string())?;
    let (_, options) = login_traffic_scopes(
        &control_url,
        username.trim().to_owned(),
        Zeroizing::new(password.clone()),
        connector.clone(),
        std::time::Duration::from_secs(10),
    )
    .await
    .map_err(|error| error.to_string())?;
    let paths = DataPaths::for_scope(DataScope::User).map_err(|error| error.to_string())?;
    let registration = register_account_traffic_scope(
        &control_url,
        deployment_id,
        username.trim().to_owned(),
        Zeroizing::new(password),
        options.default_tenant_id,
        paths.root(),
        connector,
        std::time::Duration::from_secs(10),
    )
    .await
    .map_err(|error| error.to_string())?;
    let key = format!("{}:{}", registration.user_id, registration.tenant_id);
    let mut selection = state.runtimes.lock().await;
    if !selection.runtimes.contains_key(&key) {
        let config = BridgeConfig::from_env_account(
            registration.tenant_id,
            registration.user_id,
            registration.endpoint_secret_file,
        )
        .map_err(|error| error.to_string())?;
        let secret = read_endpoint_secret(&config.endpoint_secret_file)
            .map_err(|error| error.to_string())?;
        let endpoint_key = EndpointKey::new(*secret.public().as_bytes());
        let mut runtime_config = BridgeRuntimeConfig::new(paths.bridge_database());
        runtime_config.resume_incomplete_on_start = false;
        let runtime = Arc::new(
            BridgeRuntime::start(config, runtime_config, state.passwords.clone())
                .await
                .map_err(|error| error.to_string())?,
        );
        history::forward_events(app, Arc::clone(&runtime));
        selection
            .endpoints
            .insert(endpoint_key, Arc::clone(&runtime));
        selection.runtimes.insert(key.clone(), runtime);
    }
    selection.active = key;
    let status = ScopeStatus {
        user_id: registration.user_id.to_string(),
        username: username.trim().to_owned(),
        tenant_id: registration.tenant_id.to_string(),
        team_name: registration.team_name,
    };
    selection.active_scope = Some(status.clone());
    Ok(status)
}

#[tauri::command]
pub async fn operator_use_guest_scope(
    state: tauri::State<'_, OperatorState>,
) -> Result<(), String> {
    let mut selection = state.runtimes.lock().await;
    selection.active = "guest".to_owned();
    selection.active_scope = None;
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TransferUpdate {
    id: String,
    state: &'static str,
    offset: u64,
    size: u64,
    message: Option<String>,
}

#[tauri::command]
pub async fn operator_start_transfer(
    app: tauri::AppHandle,
    state: tauri::State<'_, OperatorState>,
    code: String,
    direction: String,
    source: String,
    destination: String,
    overwrite: bool,
) -> Result<String, String> {
    if source.trim().is_empty() || destination.trim().is_empty() {
        return Err("source and destination paths are required".to_owned());
    }
    if direction != "upload" && direction != "download" {
        return Err("invalid transfer direction".to_owned());
    }
    let runtime = state.runtime().await?;
    let device_ref = runtime
        .resolve_device_code(parse_code(&code)?)
        .await
        .map_err(|error| error.to_string())?;
    let request_id = RequestId::new();
    runtime
        .prepare_transfer_record(
            request_id,
            device_ref,
            &direction,
            &source,
            &destination,
            overwrite,
        )
        .await
        .map_err(|error| error.to_string())?;
    let id = request_id.to_string();
    let task_id = id.clone();
    let transfers = Arc::clone(&state.transfers);
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        let _ = start_rx.await;
        let progress_app = app.clone();
        let progress_id = task_id.clone();
        let progress = move |offset, size| {
            let _ = progress_app.emit(
                "operator-transfer",
                TransferUpdate {
                    id: progress_id.clone(),
                    state: "running",
                    offset,
                    size,
                    message: None,
                },
            );
        };
        let result = if direction == "upload" {
            runtime
                .upload_file_with_id(
                    request_id,
                    device_ref,
                    std::path::Path::new(&source),
                    &destination,
                    overwrite,
                    progress,
                )
                .await
        } else {
            runtime
                .download_file_with_id(
                    request_id,
                    device_ref,
                    &source,
                    std::path::Path::new(&destination),
                    overwrite,
                    progress,
                )
                .await
        };
        let (state, message) = match result {
            Ok(()) => ("completed", None),
            Err(error) => ("failed", Some(error.to_string())),
        };
        let _ = app.emit(
            "operator-transfer",
            TransferUpdate {
                id: task_id.clone(),
                state,
                offset: 0,
                size: 0,
                message,
            },
        );
        transfers.lock().await.remove(&task_id);
    });
    state
        .transfers
        .lock()
        .await
        .insert(id.clone(), handle.abort_handle());
    let _ = start_tx.send(());
    Ok(id)
}

#[tauri::command]
pub async fn operator_cancel_transfer(
    app: tauri::AppHandle,
    state: tauri::State<'_, OperatorState>,
    id: String,
) -> Result<(), String> {
    let handle = state
        .transfers
        .lock()
        .await
        .remove(&id)
        .ok_or_else(|| "transfer is not running".to_owned())?;
    handle.abort();
    let request_id = id.parse::<RequestId>().map_err(|error| error.to_string())?;
    let recorded = state
        .runtime()
        .await?
        .cancel_transfer_record(request_id)
        .await
        .map_err(|error| error.to_string())?;
    if !recorded {
        return Err("local transfer stopped, but its remote outcome is not recorded".to_owned());
    }
    let _ = app.emit(
        "operator-transfer",
        TransferUpdate {
            id,
            state: "cancel_requested",
            offset: 0,
            size: 0,
            message: None,
        },
    );
    Ok(())
}

fn parse_code(code: &str) -> Result<DeviceCode, String> {
    code.parse::<DeviceCode>()
        .map_err(|_| "invalid 9-digit device code".to_owned())
}

#[tauri::command]
pub async fn operator_connect(
    state: tauri::State<'_, OperatorState>,
    code: String,
    password: String,
) -> Result<ConnectedDevice, String> {
    let code = parse_code(&code)?;
    let password = Zeroizing::new(password);
    let runtime = state.runtime().await?;
    let device_ref = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        runtime.resolve_device_code(code),
    )
    .await
    .map_err(|_| "device code resolution timed out".to_owned())?
    .map_err(|error| error.to_string())?;
    state
        .passwords
        .set_password(device_ref.device_id, password.to_string())
        .map_err(|error| error.to_string())?;
    let target = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        runtime.connect_device(device_ref),
    )
    .await
    .map_err(|_| "device connection timed out".to_owned())?
    .map_err(|error| error.to_string())?;
    let remembered = RememberedDevice {
        device_ref,
        code,
        alias: String::new(),
        os_family: target.execution.os_family,
        os_reminder: target.compact_reminder(),
    };
    runtime
        .remember_device(&remembered)
        .await
        .map_err(|error| error.to_string())?;
    state
        .local_store()
        .await?
        .save_device_password(device_ref.device_id, &password)
        .await
        .map_err(|error| error.to_string())?;
    runtime
        .resume_incomplete_for_device(device_ref)
        .await
        .map_err(|error| error.to_string())?;
    let alias = runtime
        .remembered_devices()
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|device| device.device_ref == device_ref)
        .map(|device| device.alias)
        .unwrap_or_default();
    Ok(ConnectedDevice {
        device_id: device_ref.device_id.to_string(),
        device_code: code.to_string(),
        alias,
        os_family: format!("{:?}", target.execution.os_family).to_lowercase(),
        os_reminder: target.compact_reminder(),
        connected: true,
    })
}

#[tauri::command]
pub async fn operator_connect_saved(
    state: tauri::State<'_, OperatorState>,
    code: String,
) -> Result<ConnectedDevice, String> {
    let code = parse_code(&code)?;
    let remembered = state
        .local_store()
        .await?
        .remembered_devices()
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|device| device.code == code)
        .ok_or_else(|| "saved device was not found".to_owned())?;
    let runtime = state.runtime().await?;
    let target = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        runtime.connect_device(remembered.device_ref),
    )
    .await
    .map_err(|_| "device connection timed out".to_owned())?
    .map_err(|error| error.to_string())?;
    runtime
        .remember_device(&remembered)
        .await
        .map_err(|error| error.to_string())?;
    Ok(ConnectedDevice {
        device_id: remembered.device_ref.device_id.to_string(),
        device_code: code.to_string(),
        alias: remembered.alias,
        os_family: format!("{:?}", target.execution.os_family).to_lowercase(),
        os_reminder: target.compact_reminder(),
        connected: true,
    })
}

#[tauri::command]
pub async fn operator_forget_device(
    state: tauri::State<'_, OperatorState>,
    code: String,
) -> Result<(), String> {
    let code = parse_code(&code)?;
    let local = state.local_store().await?;
    let remembered = local
        .remembered_devices()
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|device| device.code == code)
        .ok_or_else(|| "saved device was not found".to_owned())?;
    let runtimes = state
        .runtimes
        .lock()
        .await
        .runtimes
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for runtime in runtimes {
        runtime.disconnect_device(remembered.device_ref).await;
    }
    let device_id = local
        .forget_device(code)
        .await
        .map_err(|error| error.to_string())?;
    state
        .passwords
        .forget_password(device_id)
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn operator_disconnect_device(
    state: tauri::State<'_, OperatorState>,
    code: String,
) -> Result<(), String> {
    let code = parse_code(&code)?;
    let remembered = state
        .local_store()
        .await?
        .remembered_devices()
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|device| device.code == code)
        .ok_or_else(|| "saved device was not found".to_owned())?;
    state
        .runtime()
        .await?
        .disconnect_device(remembered.device_ref)
        .await;
    Ok(())
}

#[tauri::command]
pub async fn operator_presence(
    state: tauri::State<'_, OperatorState>,
    code: String,
) -> Result<u16, String> {
    let runtime = state.runtime().await?;
    let device_ref = runtime
        .resolve_device_code(parse_code(&code)?)
        .await
        .map_err(|error| error.to_string())?;
    runtime
        .current_presence(device_ref)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn operator_run_command(
    state: tauri::State<'_, OperatorState>,
    code: String,
    program: String,
    args: Vec<String>,
    cwd: Option<String>,
) -> Result<String, String> {
    if program.trim().is_empty() {
        return Err("program is required".to_owned());
    }
    let runtime = state.runtime().await?;
    let device_ref = runtime
        .resolve_device_code(parse_code(&code)?)
        .await
        .map_err(|error| error.to_string())?;
    let snapshot = runtime
        .submit_command(device_ref, RequestId::new(), program, args, cwd)
        .await
        .map_err(|error| error.to_string())?;
    state.tasks.lock().await.insert(
        snapshot.task_ref.task_id,
        (snapshot.task_ref, Arc::clone(&runtime)),
    );
    Ok(snapshot.task_ref.task_id.to_string())
}

#[tauri::command]
pub async fn operator_task(
    state: tauri::State<'_, OperatorState>,
    task_id: String,
    stdout_offset: u64,
    stderr_offset: u64,
) -> Result<TaskUpdate, String> {
    let task_id = task_id
        .parse::<TaskId>()
        .map_err(|_| "invalid task ID".to_owned())?;
    let local = state.local_store().await?;
    let saved = local
        .task_by_id(task_id)
        .await
        .map_err(|error| error.to_string())?;
    let snapshot = saved
        .snapshot
        .as_ref()
        .ok_or_else(|| "task snapshot is unavailable".to_owned())?;
    let task_ref = snapshot.task_ref;
    let runtime = state.tasks.lock().await.get(&task_id).cloned();
    let runtime = if let Some((_, runtime)) = runtime {
        Some(runtime)
    } else {
        let endpoint_key = match &snapshot.initiated_by {
            pab_protocol::OperatorRef::Account { endpoint_key, .. } => endpoint_key,
            pab_protocol::OperatorRef::Guest { guest_endpoint_key } => guest_endpoint_key,
        };
        state.runtimes.lock().await.endpoints.get(endpoint_key).cloned()
    };
    if let Some(runtime) = runtime {
        runtime
            .follow_task(task_ref)
            .await
            .map_err(|error| error.to_string())?;
        state
            .tasks
            .lock()
            .await
            .insert(task_id, (task_ref, runtime));
    }
    let record = local
        .task(task_ref)
        .await
        .map_err(|error| error.to_string())?;
    let (stdout, _) = local
        .read_output(task_ref, OutputStream::Stdout, stdout_offset, 32 * 1024)
        .await
        .map_err(|error| error.to_string())?;
    let (stderr, _) = local
        .read_output(task_ref, OutputStream::Stderr, stderr_offset, 32 * 1024)
        .await
        .map_err(|error| error.to_string())?;
    let state = record
        .snapshot
        .as_ref()
        .map(|snapshot| format!("{:?}", snapshot.state))
        .unwrap_or_else(|| "Pending".to_owned());
    Ok(TaskUpdate {
        state,
        complete: record.is_complete()
            && stdout_offset + stdout.bytes.len() as u64 >= record.stdout.available_to
            && stderr_offset + stderr.bytes.len() as u64 >= record.stderr.available_to,
        stdout: decode_command_output(&stdout.bytes),
        stderr: decode_command_output(&stderr.bytes),
        stdout_offset: stdout_offset + stdout.bytes.len() as u64,
        stderr_offset: stderr_offset + stderr.bytes.len() as u64,
    })
}

fn decode_command_output(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_owned();
    }
    // Windows console programs on Chinese systems commonly write CP936 bytes.
    // Keep the stored output and offsets as raw bytes; decode only for display.
    let (text, _, _) = encoding_rs::GBK.decode(bytes);
    text.into_owned()
}
