use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool},
};

use pab_agent_core::{
    DataPaths, DataScope, begin_device_claim, begin_personal_device_claim, tls_connector,
};
use pab_bridge::{
    BridgeConfig, BridgeLocalStore, BridgeRuntime, BridgeRuntimeConfig, MemoryDevicePasswordProvider,
    RememberedDevice,
};
use pab_protocol::{DeviceCode, OutputStream, RequestId, TaskId, TaskRef};
use serde::Serialize;
use tokio::sync::{Mutex, OnceCell};
use zeroize::Zeroizing;

pub(crate) mod history;

pub struct OperatorState {
    runtime: Mutex<Option<Arc<BridgeRuntime>>>,
    passwords: Arc<MemoryDevicePasswordProvider>,
    tasks: Mutex<HashMap<TaskId, TaskRef>>,
    events_started: AtomicBool,
    local: OnceCell<Arc<BridgeLocalStore>>,
}

impl OperatorState {
    pub fn new() -> Self {
        Self {
            runtime: Mutex::new(None),
            passwords: Arc::new(MemoryDevicePasswordProvider::new(None)),
            tasks: Mutex::new(HashMap::new()),
            events_started: AtomicBool::new(false),
            local: OnceCell::new(),
        }
    }

    async fn runtime(&self) -> Result<Arc<BridgeRuntime>, String> {
        let mut current = self.runtime.lock().await;
        if let Some(runtime) = current.as_ref() {
            return Ok(Arc::clone(runtime));
        }
        let config = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            BridgeConfig::register_guest_from_env(),
        )
        .await
        .map_err(|_| "guest registration timed out".to_owned())?
        .map_err(|error| error.to_string())?;
        let paths = DataPaths::for_scope(DataScope::User).map_err(|error| error.to_string())?;
        let mut runtime_config = BridgeRuntimeConfig::new(paths.bridge_database());
        runtime_config.resume_incomplete_on_start = false;
        let runtime = BridgeRuntime::start(
            config,
            runtime_config,
            self.passwords.clone(),
        )
        .await
        .map_err(|error| error.to_string())?;
        let runtime = Arc::new(runtime);
        *current = Some(Arc::clone(&runtime));
        Ok(runtime)
    }

    async fn local_store(&self) -> Result<Arc<BridgeLocalStore>, String> {
        let local = self
            .local
            .get_or_try_init(|| async {
                let paths = DataPaths::for_scope(DataScope::User)
                    .map_err(|error| error.to_string())?;
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
pub struct TaskUpdate {
    state: String,
    complete: bool,
    stdout: String,
    stderr: String,
    stdout_offset: u64,
    stderr_offset: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimResult {
    claim_id: String,
    owner_tenant_id: String,
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
    let runtime = state.runtime().await?;
    let device_ref = runtime
        .resolve_device_code(code)
        .await
        .map_err(|error| error.to_string())?;
    state
        .passwords
        .set_password(device_ref.device_id, password)
        .map_err(|error| error.to_string())?;
    let target = runtime
        .current_environment(device_ref)
        .await
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
    state
        .tasks
        .lock()
        .await
        .insert(snapshot.task_ref.task_id, snapshot.task_ref);
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
    let task_ref = *state
        .tasks
        .lock()
        .await
        .get(&task_id)
        .ok_or_else(|| "task is not in this desktop session".to_owned())?;
    let local = state.local_store().await?;
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
        stdout: String::from_utf8_lossy(&stdout.bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr.bytes).into_owned(),
        stdout_offset: stdout_offset + stdout.bytes.len() as u64,
        stderr_offset: stderr_offset + stderr.bytes.len() as u64,
    })
}

#[tauri::command]
pub async fn operator_claim(
    code: String,
    username: String,
    password: String,
    team_id: Option<String>,
) -> Result<ClaimResult, String> {
    if username.trim().is_empty() || password.is_empty() {
        return Err("account and password are required".to_owned());
    }
    let code = parse_code(&code)?;
    let control_url = std::env::var("PAB_CONTROL_URL")
        .map_err(|_| "PAB_CONTROL_URL is not configured".to_owned())?;
    let ca = std::env::var_os("PAB_CONTROL_CA_CERT")
        .map(std::fs::read)
        .transpose()
        .map_err(|error| error.to_string())?;
    let connector = tls_connector(ca.as_deref()).map_err(|error| error.to_string())?;
    let password = Zeroizing::new(password);
    let timeout = std::time::Duration::from_secs(10);
    let (claim_id, tenant_id) = match team_id.as_deref().filter(|value| !value.trim().is_empty()) {
        Some(team_id) => {
            let tenant_id = team_id.parse().map_err(|_| "invalid Team ID".to_owned())?;
            let claim_id = begin_device_claim(
                &control_url,
                username,
                password,
                code,
                tenant_id,
                connector,
                timeout,
            )
            .await
            .map_err(|error| error.to_string())?;
            (claim_id, tenant_id)
        }
        None => {
            begin_personal_device_claim(&control_url, username, password, code, connector, timeout)
                .await
                .map_err(|error| error.to_string())?
        }
    };
    Ok(ClaimResult {
        claim_id: claim_id.to_string(),
        owner_tenant_id: tenant_id.to_string(),
    })
}
