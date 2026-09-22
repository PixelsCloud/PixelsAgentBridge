use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use pab_bridge::{
    AuthenticatedDeviceConnection, BridgeClient, BridgeConfig, BridgeError, TaskSubscription,
};
use pab_protocol::{
    CommandTaskSpec, DeviceId, DeviceRef, DeviceTaskResponse, ExpectedEnvironment, OutputStream,
    RequestId, TaskEvent, TaskId, TaskRef, TaskSnapshot, TaskState,
};
use thiserror::Error;
use zeroize::Zeroizing;

const RETRY_INTERVAL: Duration = Duration::from_secs(3);

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
    let device_id = args
        .next()
        .ok_or(CliError::Usage)?
        .parse::<DeviceId>()
        .map_err(|error| CliError::DeviceId(error.to_string()))?;
    let config = BridgeConfig::from_env()?;
    let device_ref = DeviceRef {
        deployment_id: config.deployment_id,
        tenant_id: config.tenant_id,
        device_id,
    };
    let password_file = env::var_os("PAB_DEVICE_PASSWORD_FILE")
        .map(PathBuf::from)
        .ok_or(CliError::MissingPasswordFile)?;

    match command.as_str() {
        "command" => {
            let program = args.next().ok_or(CliError::Usage)?;
            run_command(config, device_ref, &password_file, program, args.collect()).await
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
            run_follow(config, device_ref, &password_file, task_id).await
        }
        _ => Err(CliError::Usage),
    }
}

async fn run_command(
    config: BridgeConfig,
    device_ref: DeviceRef,
    password_file: &Path,
    program: String,
    program_args: Vec<String>,
) -> Result<(), CliError> {
    let cwd = env::var("PAB_COMMAND_CWD").ok();
    let display_summary = display_summary(&program, &program_args);

    let bridge = connect_bridge(config).await?;
    let mut device = connect_device(&bridge, device_ref, password_file).await?;
    let target = loop {
        match device.get_environment().await {
            Ok(target) => break target,
            Err(error) if error.is_recoverable_connection() => {
                report_retry("reading the target environment", &error);
                device = reconnect_device(&bridge, device_ref, password_file).await?;
            }
            Err(error) => return Err(error.into()),
        }
    };
    eprintln!("pab-bridge: {}", target.compact_reminder());
    let command = CommandTaskSpec {
        program,
        args: program_args,
        cwd,
        expected_environment: ExpectedEnvironment {
            os_family: target.execution.os_family,
            environment_revision: target.execution.environment_revision.clone(),
        },
        display_summary,
    };
    let request_id = match env::var("PAB_REQUEST_ID") {
        Ok(value) => value
            .parse::<RequestId>()
            .map_err(|error| CliError::RequestId(error.to_string()))?,
        Err(env::VarError::NotPresent) => RequestId::new(),
        Err(env::VarError::NotUnicode(_)) => return Err(CliError::RequestIdEncoding),
    };
    let submitted = loop {
        match device.submit_command(request_id, command.clone()).await {
            Ok(snapshot) => break snapshot,
            Err(error) if error.is_recoverable_connection() => {
                report_retry("submitting the command", &error);
                device = reconnect_device(&bridge, device_ref, password_file).await?;
            }
            Err(error) => return Err(error.into()),
        }
    };
    eprintln!(
        "pab-bridge: task={} state={:?}",
        submitted.task_ref.task_id, submitted.state
    );
    let final_snapshot = follow_task(
        &bridge,
        &mut device,
        submitted.task_ref,
        device_ref,
        password_file,
    )
    .await?;
    device.close();
    bridge.shutdown().await?;
    eprintln!(
        "pab-bridge: task={} final={:?}",
        final_snapshot.task_ref.task_id, final_snapshot.state
    );
    match final_snapshot.state {
        TaskState::Succeeded | TaskState::Cancelled => Ok(()),
        _ => Err(CliError::RemoteTask(Box::new(final_snapshot))),
    }
}

async fn run_follow(
    config: BridgeConfig,
    device_ref: DeviceRef,
    password_file: &Path,
    task_id: TaskId,
) -> Result<(), CliError> {
    let bridge = connect_bridge(config).await?;
    let mut device = connect_device(&bridge, device_ref, password_file).await?;
    let task_ref = TaskRef {
        device_ref,
        task_id,
    };
    let snapshot = loop {
        match device.get_task(task_ref).await {
            Ok(snapshot) => break snapshot,
            Err(error) if error.is_recoverable_connection() => {
                report_retry("reading the task", &error);
                device = reconnect_device(&bridge, device_ref, password_file).await?;
            }
            Err(error) => return Err(error.into()),
        }
    };
    eprintln!(
        "pab-bridge: {}",
        snapshot.target_context().compact_reminder()
    );
    eprintln!(
        "pab-bridge: task={} state={:?}",
        snapshot.task_ref.task_id, snapshot.state
    );
    let final_snapshot =
        follow_task(&bridge, &mut device, task_ref, device_ref, password_file).await?;
    device.close();
    bridge.shutdown().await?;
    eprintln!(
        "pab-bridge: task={} final={:?}",
        final_snapshot.task_ref.task_id, final_snapshot.state
    );
    match final_snapshot.state {
        TaskState::Succeeded | TaskState::Cancelled => Ok(()),
        _ => Err(CliError::RemoteTask(Box::new(final_snapshot))),
    }
}

async fn follow_task(
    bridge: &BridgeClient,
    device: &mut AuthenticatedDeviceConnection,
    task_ref: TaskRef,
    device_ref: DeviceRef,
    password_file: &Path,
) -> Result<TaskSnapshot, CliError> {
    let mut event_seq = 0_u64;
    let mut stdout_offset = 0_u64;
    let mut stderr_offset = 0_u64;
    let mut cancel_requested = false;
    loop {
        if cancel_requested {
            match device
                .cancel_task(task_ref, "local operator requested cancellation".to_owned())
                .await
            {
                Ok(_) => {
                    eprintln!("pab-bridge: cancellation accepted");
                    cancel_requested = false;
                }
                Err(BridgeError::RemoteTask { .. }) => cancel_requested = false,
                Err(error) if error.is_recoverable_connection() => {
                    report_retry("cancelling the task", &error);
                    *device = reconnect_device(bridge, device_ref, password_file).await?;
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
        }
        let mut subscription = match device
            .subscribe_task(task_ref, event_seq, stdout_offset, stderr_offset)
            .await
        {
            Ok(subscription) => subscription,
            Err(error) if error.is_recoverable_connection() => {
                report_retry("subscribing to the task", &error);
                *device = reconnect_device(bridge, device_ref, password_file).await?;
                continue;
            }
            Err(error) => return Err(error.into()),
        };

        match consume_subscription(
            &mut subscription,
            &mut event_seq,
            &mut stdout_offset,
            &mut stderr_offset,
            !cancel_requested,
        )
        .await
        {
            SubscriptionEnd::Finished(snapshot) => return Ok(*snapshot),
            SubscriptionEnd::Cancel => {
                cancel_requested = true;
            }
            SubscriptionEnd::Disconnected(error) => {
                report_retry("following the task", &error);
                tokio::time::sleep(RETRY_INTERVAL).await;
                *device = connect_device(bridge, device_ref, password_file).await?;
            }
            SubscriptionEnd::Failed(error) => return Err(error),
        }
    }
}

async fn consume_subscription(
    subscription: &mut TaskSubscription,
    event_seq: &mut u64,
    stdout_offset: &mut u64,
    stderr_offset: &mut u64,
    listen_for_cancel: bool,
) -> SubscriptionEnd {
    loop {
        tokio::select! {
            response = subscription.next() => {
                let response = match response {
                    Ok(response) => response,
                    Err(error) if error.is_recoverable_connection() => {
                        return SubscriptionEnd::Disconnected(error);
                    }
                    Err(error) => return SubscriptionEnd::Failed(error.into()),
                };
                match handle_response(response, event_seq, stdout_offset, stderr_offset) {
                    Ok(Some(snapshot)) => return SubscriptionEnd::Finished(Box::new(snapshot)),
                    Ok(None) => {}
                    Err(error) => return SubscriptionEnd::Failed(error),
                }
            }
            signal = tokio::signal::ctrl_c(), if listen_for_cancel => {
                match signal {
                    Ok(()) => return SubscriptionEnd::Cancel,
                    Err(error) => return SubscriptionEnd::Failed(CliError::Signal(error)),
                }
            }
        }
    }
}

fn handle_response(
    response: DeviceTaskResponse,
    event_seq: &mut u64,
    stdout_offset: &mut u64,
    stderr_offset: &mut u64,
) -> Result<Option<TaskSnapshot>, CliError> {
    match response {
        DeviceTaskResponse::Event { event } => {
            accept_event(event, event_seq)?;
        }
        DeviceTaskResponse::Output { chunk, .. } => {
            let expected = match chunk.stream {
                OutputStream::Stdout => stdout_offset,
                OutputStream::Stderr => stderr_offset,
            };
            if chunk.offset != *expected {
                return Err(CliError::OutputGap {
                    expected: *expected,
                    actual: chunk.offset,
                });
            }
            match chunk.stream {
                OutputStream::Stdout => {
                    io::stdout().write_all(&chunk.bytes)?;
                    io::stdout().flush()?;
                }
                OutputStream::Stderr => {
                    io::stderr().write_all(&chunk.bytes)?;
                    io::stderr().flush()?;
                }
            }
            *expected = expected
                .checked_add(u64::try_from(chunk.bytes.len()).map_err(|_| CliError::Offset)?)
                .ok_or(CliError::Offset)?;
        }
        DeviceTaskResponse::CaughtUp { snapshot } => {
            if snapshot.state.is_terminal()
                && *event_seq == snapshot.latest_event_seq
                && *stdout_offset == snapshot.output.stdout.available_to
                && *stderr_offset == snapshot.output.stderr.available_to
                && snapshot.output.stdout.complete
                && snapshot.output.stderr.complete
            {
                return Ok(Some(*snapshot));
            }
        }
        DeviceTaskResponse::OutputChanged { .. } => {}
        response => return Err(CliError::UnexpectedResponse(format!("{response:?}"))),
    }
    Ok(None)
}

fn accept_event(event: TaskEvent, event_seq: &mut u64) -> Result<(), CliError> {
    if event.seq <= *event_seq {
        return Ok(());
    }
    let expected = event_seq.checked_add(1).ok_or(CliError::Offset)?;
    if event.seq != expected {
        return Err(CliError::EventGap {
            expected,
            actual: event.seq,
        });
    }
    *event_seq = event.seq;
    eprintln!(
        "pab-bridge: task={} seq={} event={:?}",
        event.task_ref.task_id, event.seq, event.kind
    );
    Ok(())
}

async fn connect_bridge(config: BridgeConfig) -> Result<BridgeClient, CliError> {
    loop {
        match BridgeClient::connect(config.clone()).await {
            Ok(bridge) => return Ok(bridge),
            Err(error) if error.is_recoverable_connection() => {
                report_retry("starting Bridge", &error);
                tokio::time::sleep(RETRY_INTERVAL).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

async fn reconnect_device(
    bridge: &BridgeClient,
    device_ref: DeviceRef,
    password_file: &Path,
) -> Result<AuthenticatedDeviceConnection, CliError> {
    tokio::time::sleep(RETRY_INTERVAL).await;
    connect_device(bridge, device_ref, password_file).await
}

async fn connect_device(
    bridge: &BridgeClient,
    device_ref: DeviceRef,
    password_file: &Path,
) -> Result<AuthenticatedDeviceConnection, CliError> {
    loop {
        let password = read_password(password_file)?;
        match bridge.connect_device(device_ref, password).await {
            Ok(connection) => return Ok(connection),
            Err(error) if error.is_recoverable_connection() => {
                report_retry("connecting to the device", &error);
                tokio::time::sleep(RETRY_INTERVAL).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn read_password(path: &Path) -> Result<Zeroizing<String>, CliError> {
    let bytes = fs::read(path).map_err(|source| CliError::PasswordFile {
        path: path.to_owned(),
        source,
    })?;
    let mut password = String::from_utf8(bytes).map_err(|_| CliError::PasswordEncoding)?;
    let new_length = password.trim_end_matches(['\r', '\n']).len();
    password.truncate(new_length);
    if password.is_empty() {
        return Err(CliError::EmptyPassword);
    }
    Ok(Zeroizing::new(password))
}

fn display_summary(program: &str, args: &[String]) -> String {
    let mut summary = program.to_owned();
    if !args.is_empty() {
        summary.push_str(" …");
    }
    summary.chars().take(512).collect()
}

fn report_retry(action: &str, error: &BridgeError) {
    eprintln!(
        "pab-bridge: {action} failed: {error}; retrying in {} ms",
        RETRY_INTERVAL.as_millis()
    );
}

enum SubscriptionEnd {
    Finished(Box<TaskSnapshot>),
    Cancel,
    Disconnected(BridgeError),
    Failed(CliError),
}

#[derive(Debug, Error)]
enum CliError {
    #[error(
        "usage: pab-bridge command <device-id> <program> [argument ...]\n       pab-bridge follow <device-id> <task-id>"
    )]
    Usage,
    #[error("device ID is invalid: {0}")]
    DeviceId(String),
    #[error("task ID is invalid: {0}")]
    TaskId(String),
    #[error("PAB_REQUEST_ID is invalid: {0}")]
    RequestId(String),
    #[error("PAB_REQUEST_ID must contain UTF-8 text")]
    RequestIdEncoding,
    #[error(transparent)]
    Config(#[from] pab_bridge::BridgeConfigError),
    #[error(transparent)]
    Bridge(#[from] BridgeError),
    #[error("PAB_DEVICE_PASSWORD_FILE must be set")]
    MissingPasswordFile,
    #[error("device password file {path} could not be read: {source}")]
    PasswordFile { path: PathBuf, source: io::Error },
    #[error("device password file must contain UTF-8 text")]
    PasswordEncoding,
    #[error("device password file is empty")]
    EmptyPassword,
    #[error("task event stream has a gap: expected {expected}, received {actual}")]
    EventGap { expected: u64, actual: u64 },
    #[error("task output has a gap: expected offset {expected}, received {actual}")]
    OutputGap { expected: u64, actual: u64 },
    #[error("task output offset is too large")]
    Offset,
    #[error("unexpected task response: {0}")]
    UnexpectedResponse(String),
    #[error("local shutdown signal failed: {0}")]
    Signal(io::Error),
    #[error("remote task ended in state {0:?}")]
    RemoteTask(Box<TaskSnapshot>),
    #[error("local output failed: {0}")]
    Output(#[from] io::Error),
}
