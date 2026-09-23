use std::{
    env,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
};

use pab_agent_core::{DataPaths, DataScope};
use pab_bridge::{
    BridgeConfig, BridgeRuntime, BridgeRuntimeConfig, FileDevicePasswordProvider, RuntimeError,
};
use pab_protocol::{DeviceCode, OutputStream, RequestId, TaskId, TaskRef, TaskSnapshot, TaskState};
use thiserror::Error;
use tokio::sync::broadcast;

const OUTPUT_READ_BYTES: u32 = 64 * 1024;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pab-bridge: {error}");
            ExitCode::FAILURE
        }
    }
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
    let config = BridgeConfig::from_env()?;
    let password_file = env::var_os("PAB_DEVICE_PASSWORD_FILE")
        .map(PathBuf::from)
        .ok_or(CliError::MissingPasswordFile)?;
    let database_file = match env::var_os("PAB_BRIDGE_DATABASE") {
        Some(path) => PathBuf::from(path),
        None => DataPaths::for_scope(DataScope::User)?.bridge_database(),
    };
    let runtime = BridgeRuntime::start(
        config,
        BridgeRuntimeConfig::new(database_file),
        Arc::new(FileDevicePasswordProvider::new(password_file)),
    )
    .await?;
    let mut events = runtime.subscribe();
    let device_ref = runtime.resolve_device_code(device_code).await?;

    let initial = match command.as_str() {
        "command" => {
            let program = args.next().ok_or(CliError::Usage)?;
            let command_args = args.collect::<Vec<_>>();
            let request_id = request_id()?;
            let cwd = env::var("PAB_COMMAND_CWD").ok();
            runtime
                .submit_command(device_ref, request_id, program, command_args, cwd)
                .await
        }
        "follow" => {
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
    DataPath(#[from] pab_agent_core::DataPathError),
    #[error(
        "usage: pab-bridge command <9-digit-device-code> <program> [argument ...]\n       pab-bridge follow <9-digit-device-code> <task-id>"
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
