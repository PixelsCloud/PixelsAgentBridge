use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool},
};

use pab_agent_core::{DataPaths, DataScope, read_endpoint_secret};
use pab_bridge::{
    BridgeConfig, BridgeLocalStore, BridgeRuntime, BridgeRuntimeConfig, ConnectionPath,
    MemoryDevicePasswordProvider, RememberedDevice, SqliteDevicePasswordProvider,
};
use pab_protocol::{DeviceCode, EndpointKey, OutputStream, RequestId, TaskId, TaskRef};
use serde::Serialize;
use tokio::sync::{Mutex, OnceCell};
use zeroize::Zeroizing;

pub(crate) mod directory;
pub(crate) mod execution;
pub(crate) mod history;
pub(crate) mod screenshot;
pub(crate) mod terminal;
pub(crate) mod transfer;
pub(crate) mod windows;

pub struct OperatorState {
    runtimes: Mutex<RuntimeSelection>,
    account_changes: Mutex<()>,
    passwords: Arc<MemoryDevicePasswordProvider>,
    tasks: Mutex<HashMap<TaskId, (TaskRef, Arc<BridgeRuntime>)>>,
    terminal_runtimes: Mutex<HashMap<RequestId, Arc<BridgeRuntime>>>,
    execution_queries: Mutex<HashMap<RequestId, execution::QueryOwner>>,
    transfers: Arc<Mutex<HashMap<RequestId, Arc<transfer::TransferOwner>>>>,
    events_started: AtomicBool,
    local: OnceCell<Arc<BridgeLocalStore>>,
}

struct RuntimeSelection {
    active: String,
    runtimes: HashMap<String, Arc<BridgeRuntime>>,
    endpoints: HashMap<EndpointKey, Arc<BridgeRuntime>>,
}

impl OperatorState {
    pub fn new() -> Self {
        let credential_database = DataPaths::for_scope(DataScope::User)
            .expect("could not find user credential database path")
            .bridge_database();
        Self {
            account_changes: Mutex::new(()),
            runtimes: Mutex::new(RuntimeSelection {
                active: "guest".to_owned(),
                runtimes: HashMap::new(),
                endpoints: HashMap::new(),
            }),
            passwords: Arc::new(MemoryDevicePasswordProvider::new(Some(Box::new(
                SqliteDevicePasswordProvider::new(credential_database),
            )))),
            tasks: Mutex::new(HashMap::new()),
            terminal_runtimes: Mutex::new(HashMap::new()),
            execution_queries: Mutex::new(HashMap::new()),
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
        let path = runtime
            .connection_path(device.device_ref)
            .await
            .map(|path| match path {
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
                name: presence
                    .as_ref()
                    .map(|value| value.name.clone())
                    .unwrap_or_default(),
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
    execution_identity: Option<pab_protocol::ExecutionIdentity>,
    state: String,
    complete: bool,
    stdout: String,
    stderr: String,
    stdout_offset: u64,
    stderr_offset: u64,
    decoding_replacements: bool,
    output_gap: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeStatus {
    user_id: String,
    username: String,
    server_admin: bool,
    revision: u64,
}

fn account_status(state: &pab_agent_core::account::AccountState) -> Option<ScopeStatus> {
    state.user.as_ref().map(|user| ScopeStatus {
        user_id: user.id.to_string(),
        username: user.username.clone(),
        server_admin: user.server_admin,
        revision: state.revision,
    })
}

#[tauri::command]
pub async fn operator_current_traffic_scope() -> Result<Option<ScopeStatus>, String> {
    let state = pab_agent_core::account::AccountStore::from_env()
        .and_then(|store| store.read())
        .map_err(|error| error.to_string())?;
    Ok(account_status(&state))
}

async fn sign_in(
    state: &OperatorState,
    reporting: &crate::mcp_reporting::McpReportingState,
    username: String,
    password: String,
    register: bool,
) -> Result<ScopeStatus, String> {
    use pab_agent_core::account::{AccountClient, AccountStore};
    let _guard = state.account_changes.lock().await;
    let client = AccountClient::from_env().map_err(|error| error.to_string())?;
    let store = AccountStore::from_env().map_err(|error| error.to_string())?;
    let session = client
        .login(username.trim(), Zeroizing::new(password), register)
        .await
        .map_err(|error| error.to_string())?;
    let snapshot = client
        .save_session(&store, session)
        .await
        .map_err(|error| {
            if register {
                format!(
                    "Account created, but sign-in could not be saved. Please sign in again: {error}"
                )
            } else {
                error.to_string()
            }
        })?;
    reporting.account_changed(snapshot.revision);
    pab_agent_core::account::notify_account_change(snapshot.revision);
    tokio::spawn(async move {
        let _ = client.flush_logouts(&store).await;
    });
    account_status(&snapshot).ok_or_else(|| "account state was not saved".to_owned())
}

#[tauri::command]
pub async fn operator_login_account(
    state: tauri::State<'_, OperatorState>,
    reporting: tauri::State<'_, crate::mcp_reporting::McpReportingState>,
    username: String,
    password: String,
) -> Result<ScopeStatus, String> {
    sign_in(&state, &reporting, username, password, false).await
}

#[tauri::command]
pub async fn operator_register_account(
    state: tauri::State<'_, OperatorState>,
    reporting: tauri::State<'_, crate::mcp_reporting::McpReportingState>,
    username: String,
    password: String,
) -> Result<ScopeStatus, String> {
    sign_in(&state, &reporting, username, password, true).await
}

#[tauri::command]
pub async fn operator_use_guest_scope(
    state: tauri::State<'_, OperatorState>,
    reporting: tauri::State<'_, crate::mcp_reporting::McpReportingState>,
) -> Result<(), String> {
    let _guard = state.account_changes.lock().await;
    let store =
        pab_agent_core::account::AccountStore::from_env().map_err(|error| error.to_string())?;
    let snapshot = store.logout().map_err(|error| error.to_string())?;
    reporting.account_changed(snapshot.revision);
    pab_agent_core::account::notify_account_change(snapshot.revision);
    tokio::spawn(async move {
        if let Ok(client) = pab_agent_core::account::AccountClient::from_env() {
            let _ = client.flush_logouts(&store).await;
        }
    });
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
        os_family: Some(target.execution.os_family),
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
    execution: Option<pab_protocol::ExecutionSelection>,
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
        .submit_command_as(
            device_ref,
            RequestId::new(),
            program,
            args,
            cwd,
            execution.unwrap_or_default(),
        )
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
    encoding: Option<pab_bridge::output_text::OutputEncoding>,
) -> Result<TaskUpdate, String> {
    let encoding = encoding.unwrap_or_default();
    encoding.validate_offset(stdout_offset)?;
    encoding.validate_offset(stderr_offset)?;
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
        state
            .runtimes
            .lock()
            .await
            .endpoints
            .get(endpoint_key)
            .cloned()
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
    let stdout_start = encoding
        .align_start(stdout_offset.max(record.stdout.retained_from))
        .min(record.stdout.available_to);
    let stderr_start = encoding
        .align_start(stderr_offset.max(record.stderr.retained_from))
        .min(record.stderr.available_to);
    let (stdout, stdout_range) = local
        .read_output(task_ref, OutputStream::Stdout, stdout_start, 32 * 1024)
        .await
        .map_err(|error| error.to_string())?;
    let (stderr, stderr_range) = local
        .read_output(task_ref, OutputStream::Stderr, stderr_start, 32 * 1024)
        .await
        .map_err(|error| error.to_string())?;
    let state = record
        .snapshot
        .as_ref()
        .map(|snapshot| format!("{:?}", snapshot.state))
        .unwrap_or_else(|| "Pending".to_owned());
    let stdout_text = pab_bridge::output_text::decode_output(
        &stdout.bytes,
        encoding,
        stdout_range.complete
            && stdout.offset + stdout.bytes.len() as u64 >= stdout_range.available_to,
    );
    let stderr_text = pab_bridge::output_text::decode_output(
        &stderr.bytes,
        encoding,
        stderr_range.complete
            && stderr.offset + stderr.bytes.len() as u64 >= stderr_range.available_to,
    );
    let next_stdout = stdout.offset + stdout_text.consumed as u64;
    let next_stderr = stderr.offset + stderr_text.consumed as u64;
    Ok(TaskUpdate {
        execution_identity: record
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.execution_context.identity.clone()),
        state,
        complete: record.is_complete()
            && next_stdout >= stdout_range.available_to
            && next_stderr >= stderr_range.available_to,
        decoding_replacements: stdout_text.replacements || stderr_text.replacements,
        output_gap: stdout_start > stdout_offset || stderr_start > stderr_offset,
        stdout: stdout_text.text,
        stderr: stderr_text.text,
        stdout_offset: next_stdout,
        stderr_offset: next_stderr,
    })
}
