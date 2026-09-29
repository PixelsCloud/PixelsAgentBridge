use std::{
    env,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
};

use pab_agent_core::{DataPaths, DataScope, begin_personal_device_claim, tls_connector};
use pab_bridge::{
    BridgeConfig, BridgeRuntime, BridgeRuntimeConfig, DevicePasswordProvider,
    DirectoryDevicePasswordProvider, FileDevicePasswordProvider, RuntimeError,
};
use pab_protocol::{DeviceCode, OutputStream, RequestId, TaskId, TaskRef, TaskSnapshot, TaskState};
use thiserror::Error;
use tokio::sync::broadcast;

const OUTPUT_READ_BYTES: u32 = 64 * 1024;

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(error) = init_logging() {
        eprintln!("pab-bridge: {error}");
        return ExitCode::FAILURE;
    }
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "bridge failed");
            eprintln!("pab-bridge: {error}");
            ExitCode::FAILURE
        }
    }
}

fn init_logging() -> Result<(), Box<dyn std::error::Error>> {
    let paths = DataPaths::for_scope(DataScope::User)?;
    pab_logging::init("bridge", paths.root())?;
    Ok(())
}

async fn run() -> Result<(), CliError> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(CliError::Usage);
    };
    let device_code = args
        .next()
        .ok_or(CliError::Usage)?
        .parse::<DeviceCode>()
        .map_err(|error| CliError::DeviceCode(error.to_string()))?;
    if command == "claim" {
        if args.next().is_some() {
            return Err(CliError::Usage);
        }
        let username =
            env::var("PAB_ACCOUNT_USERNAME").map_err(|_| CliError::MissingAccountUsername)?;
        let password_path =
            env::var_os("PAB_ACCOUNT_PASSWORD_FILE").ok_or(CliError::MissingAccountPassword)?;
        let password = zeroize::Zeroizing::new(
            std::fs::read_to_string(password_path)?
                .trim_end()
                .to_owned(),
        );
        let url = env::var("PAB_CONTROL_URL").map_err(|_| CliError::MissingControlUrl)?;
        let ca = env::var_os("PAB_CONTROL_CA_CERT")
            .map(std::fs::read)
            .transpose()?;
        let (claim_id, _) = begin_personal_device_claim(
            &url,
            username,
            password,
            device_code,
            tls_connector(ca.as_deref())?,
            std::time::Duration::from_secs(10),
        )
        .await?;
        println!("Claim request: {claim_id}");
        println!("On the device run: pab-executor approve-claim {claim_id}");
        return Ok(());
    }
    let guest = command.starts_with("guest-");
    let config = if guest {
        BridgeConfig::register_guest_from_env().await?
    } else {
        BridgeConfig::from_env()?
    };
    let passwords: Arc<dyn DevicePasswordProvider> =
        if let Some(directory) = env::var_os("PAB_DEVICE_PASSWORD_DIR") {
            Arc::new(DirectoryDevicePasswordProvider::new(PathBuf::from(
                directory,
            )))
        } else {
            let password_file = env::var_os("PAB_DEVICE_PASSWORD_FILE")
                .map(PathBuf::from)
                .ok_or(CliError::MissingPasswordFile)?;
            Arc::new(FileDevicePasswordProvider::new(password_file))
        };
    let database_file = match env::var_os("PAB_BRIDGE_DATABASE") {
        Some(path) => PathBuf::from(path),
        None => DataPaths::for_scope(DataScope::User)?.bridge_database(),
    };
    let runtime =
        BridgeRuntime::start(config, BridgeRuntimeConfig::new(database_file), passwords).await?;
    let mut events = runtime.subscribe();
    let device_ref = runtime.resolve_device_code(device_code).await?;

    if matches!(
        command.as_str(),
        "transfer-status" | "guest-transfer-status"
    ) {
        let request_id = args
            .next()
            .ok_or(CliError::Usage)?
            .parse::<RequestId>()
            .map_err(|error| CliError::RequestId(error.to_string()))?;
        if args.next().is_some() {
            return Err(CliError::Usage);
        }
        let snapshot = runtime.transfer_status(device_ref, request_id).await?;
        println!(
            "request={} direction={} state={} offset={} size={} sha256={} finished_at_ms={}",
            snapshot.request_id,
            snapshot.direction,
            snapshot.state,
            snapshot.offset,
            snapshot.size,
            snapshot.sha256.as_deref().unwrap_or("unknown"),
            snapshot
                .finished_at_unix_ms
                .map_or("unknown".to_owned(), |value| value.to_string())
        );
        runtime.shutdown().await?;
        return Ok(());
    }

    let initial = match command.as_str() {
        "command" | "guest-command" => {
            let program = args.next().ok_or(CliError::Usage)?;
            let command_args = args.collect::<Vec<_>>();
            let request_id = request_id()?;
            let cwd = env::var("PAB_COMMAND_CWD").ok();
            runtime
                .submit_command(device_ref, request_id, program, command_args, cwd)
                .await
        }
        "follow" | "guest-follow" => {
            let task_id = args
                .next()
                .ok_or(CliError::Usage)?
                .parse::<TaskId>()
                .map_err(|error| CliError::TaskId(error.to_string()))?;
            if args.next().is_some() {
                return Err(CliError::Usage);
            }
            runtime
                .follow_task(TaskRef {
                    device_ref,
                    task_id,
                })
                .await
        }
        _ => return Err(CliError::Usage),
    };

    let result = match initial {
        Ok(snapshot) => {
            eprintln!(
                "pab-bridge: {}",
                snapshot.target_context().compact_reminder()
            );
            eprintln!(
                "pab-bridge: task={} state={:?}",
                snapshot.task_ref.task_id, snapshot.state
            );
            watch_task(&runtime, &mut events, snapshot.task_ref).await
        }
        Err(error) => Err(error.into()),
    };
    runtime.shutdown().await?;
    let final_snapshot = result?;
    eprintln!(
        "pab-bridge: task={} final={:?}",
        final_snapshot.task_ref.task_id, final_snapshot.state
    );
    match final_snapshot.state {
        TaskState::Succeeded | TaskState::Cancelled => Ok(()),
        _ => Err(CliError::RemoteTask(Box::new(final_snapshot))),
    }
}

async fn watch_task(
    runtime: &BridgeRuntime,
    events: &mut broadcast::Receiver<pab_bridge::RuntimeEvent>,
    task_ref: TaskRef,
) -> Result<TaskSnapshot, CliError> {
    let initial = runtime.task(task_ref).await?;
    let mut event_seq = 0_u64;
    let mut stdout_offset = initial.stdout.retained_from;
    let mut stderr_offset = initial.stderr.retained_from;
    let mut cancel_requested = false;
    loop {
        for event in runtime.events_after(task_ref, event_seq).await? {
            event_seq = event.seq;
            eprintln!(
                "pab-bridge: task={} seq={} event={:?}",
                task_ref.task_id, event.seq, event.kind
            );
        }
        drain_output(runtime, task_ref, OutputStream::Stdout, &mut stdout_offset).await?;
        drain_output(runtime, task_ref, OutputStream::Stderr, &mut stderr_offset).await?;
        let record = runtime.task(task_ref).await?;
        if record.is_complete() {
            return record.snapshot.ok_or(CliError::MissingSnapshot);
        }

        tokio::select! {
            event = events.recv() => {
                match event {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return Err(CliError::EventChannelClosed),
                }
            }
            signal = tokio::signal::ctrl_c(), if !cancel_requested => {
                signal.map_err(CliError::Signal)?;
                cancel_requested = true;
                match runtime
                    .cancel_task(task_ref, "local operator requested cancellation".to_owned())
                    .await
                {
                    Ok(_) => eprintln!("pab-bridge: cancellation accepted"),
                    Err(error) => eprintln!("pab-bridge: cancellation failed: {error}"),
                }
            }
        }
    }
}

async fn drain_output(
    runtime: &BridgeRuntime,
    task_ref: TaskRef,
    stream: OutputStream,
    offset: &mut u64,
) -> Result<(), CliError> {
    loop {
        let record = runtime.task(task_ref).await?;
        let range = match stream {
            OutputStream::Stdout => record.stdout,
            OutputStream::Stderr => record.stderr,
        };
        if *offset < range.retained_from {
            eprintln!(
                "pab-bridge: {} output before offset {} is no longer retained",
                stream_name(stream),
                range.retained_from
            );
            *offset = range.retained_from;
        }
        if *offset >= range.available_to {
            return Ok(());
        }
        let (chunk, _) = runtime
            .read_output(task_ref, stream, *offset, OUTPUT_READ_BYTES)
            .await?;
        if chunk.bytes.is_empty() {
            return Ok(());
        }
        match stream {
            OutputStream::Stdout => {
                io::stdout().write_all(&chunk.bytes)?;
                io::stdout().flush()?;
            }
            OutputStream::Stderr => {
                io::stderr().write_all(&chunk.bytes)?;
                io::stderr().flush()?;
            }
        }
        *offset = offset
            .checked_add(u64::try_from(chunk.bytes.len()).map_err(|_| CliError::Offset)?)
            .ok_or(CliError::Offset)?;
    }
}

fn request_id() -> Result<RequestId, CliError> {
    match env::var("PAB_REQUEST_ID") {
        Ok(value) => value
            .parse::<RequestId>()
            .map_err(|error| CliError::RequestId(error.to_string())),
        Err(env::VarError::NotPresent) => Ok(RequestId::new()),
        Err(env::VarError::NotUnicode(_)) => Err(CliError::RequestIdEncoding),
    }
}

const fn stream_name(stream: OutputStream) -> &'static str {
    match stream {
        OutputStream::Stdout => "stdout",
        OutputStream::Stderr => "stderr",
    }
}

#[derive(Debug, Error)]
enum CliError {
    #[error(transparent)]
    Claim(#[from] pab_agent_core::ClaimError),
    #[error("PAB_ACCOUNT_USERNAME must be set")]
    MissingAccountUsername,
    #[error("PAB_ACCOUNT_PASSWORD_FILE must be set")]
    MissingAccountPassword,
    #[error(transparent)]
    GuestConfig(#[from] pab_bridge::GuestConfigError),
    #[error(transparent)]
    Tls(#[from] pab_agent_core::TlsConnectorError),
    #[error("PAB_CONTROL_URL must be set")]
    MissingControlUrl,
    #[error(transparent)]
    DataPath(#[from] pab_agent_core::DataPathError),
    #[error(
        "usage: pab-bridge command|guest-command <9-digit-device-code> <program> [argument ...]\n       pab-bridge follow|guest-follow <9-digit-device-code> <task-id>\n       pab-bridge transfer-status|guest-transfer-status <9-digit-device-code> <request-id>\n       pab-bridge claim <9-digit-device-code>"
    )]
    Usage,
    #[error("device code is invalid: {0}")]
    DeviceCode(String),
    #[error("task ID is invalid: {0}")]
    TaskId(String),
    #[error("PAB_REQUEST_ID is invalid: {0}")]
    RequestId(String),
    #[error("PAB_REQUEST_ID must contain UTF-8 text")]
    RequestIdEncoding,
    #[error(transparent)]
    Config(#[from] pab_bridge::BridgeConfigError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("PAB_DEVICE_PASSWORD_FILE must be set")]
    MissingPasswordFile,
    #[error("the local task record has no remote snapshot")]
    MissingSnapshot,
    #[error("the Bridge Runtime event channel closed")]
    EventChannelClosed,
    #[error("local shutdown signal failed: {0}")]
    Signal(io::Error),
    #[error("task output offset is too large")]
    Offset,
    #[error("remote task ended in state {0:?}")]
    RemoteTask(Box<TaskSnapshot>),
    #[error("local output failed: {0}")]
    Output(#[from] io::Error),
}
