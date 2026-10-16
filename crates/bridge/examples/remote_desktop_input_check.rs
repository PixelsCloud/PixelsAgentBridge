//! Explicit acceptance driver. Events arrive only over stdin, never argv or
//! logs; stdout contains readiness/ACKs without key values or input contents.
use pab_agent_core::DataPaths;
use pab_bridge::{BridgeConfig, BridgeRuntime, BridgeRuntimeConfig, SqliteDevicePasswordProvider};
use pab_desktop_media::NativeViewer;
use pab_protocol::RemoteDesktopInput;
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, BufReader};

type Error = Box<dyn std::error::Error + Send + Sync>;
#[derive(serde::Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Enable { enabled: bool },
    Input { event: RemoteDesktopInput },
    Close,
}
fn main() -> std::process::ExitCode {
    if std::env::args().nth(1).as_deref() == Some("--media-worker") {
        return std::process::ExitCode::from(
            pab_desktop_media::worker_main(std::env::args().skip(2)) as u8,
        );
    }
    let result = (|| -> Result<(), Error> {
        let (args, values) = pab_service_config::client::startup(
            pab_service_config::client::Role::Operator,
            std::env::args_os().skip(1),
        )?;
        for (key, value) in values {
            // Single-threaded startup, before any runtime/worker exists.
            unsafe {
                std::env::set_var(key, value);
            }
        }
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()?
            .block_on(async { tokio::time::timeout(Duration::from_secs(120), run(args)).await? })
    })();
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("input acceptance failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
async fn run(args: Vec<String>) -> Result<(), Error> {
    if args.len() != 1 {
        return Err("usage: remote_desktop_input_check [--config FILE] DEVICE_CODE".into());
    }
    let temp = tempfile::tempdir()?;
    let config = BridgeConfig::register_guest_with_secret(temp.path().join("operator.key")).await?;
    let passwords = Arc::new(SqliteDevicePasswordProvider::new(
        DataPaths::for_operator()?.bridge_database(),
    ));
    let mut options = BridgeRuntimeConfig::new(temp.path().join("runtime.sqlite3"));
    options.resume_incomplete_on_start = false;
    let runtime = BridgeRuntime::start(config, options, passwords).await?;
    let result = drive(&runtime, args[0].parse()?).await;
    runtime.shutdown().await?;
    result
}
async fn drive(runtime: &BridgeRuntime, code: pab_protocol::DeviceCode) -> Result<(), Error> {
    let device = runtime.resolve_device_code(code).await?;
    let mut session = runtime.open_remote_desktop(device, None, 15).await?;
    if !session.opened().input_supported {
        return Err("target does not support input".into());
    }
    let mut viewer = NativeViewer::open(
        &std::env::current_exe()?,
        &["--media-worker".into()],
        &session.opened().config,
        false,
    )
    .await?;
    let first = session.next_frame(Duration::from_secs(10)).await?;
    viewer.present(&first).await?;
    session.report_presented(&first.header)?;
    println!(
        "{}",
        serde_json::json!({"ready":true,"screen":session.opened().config.screen})
    );
    let control = session.input_control();
    let video = async {
        loop {
            let frame = session.next_frame_when_changed().await?;
            viewer.present(&frame).await?;
            session.report_presented(&frame.header)?;
        }
        #[allow(unreachable_code)]
        Ok::<(), Error>(())
    };
    let inputs = async {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Some(line) = lines.next_line().await? {
            if line.len() > 1024 {
                return Err("input acceptance command too long".into());
            }
            // Parse errors are deliberately generic; serde errors can echo values.
            let command: Command =
                serde_json::from_str(&line).map_err(|_| "invalid input acceptance command")?;
            match command {
                Command::Enable { enabled } => control.set_enabled(enabled).await?,
                Command::Input { event } => control.send(event).await?,
                Command::Close => break,
            }
            println!("{{\"applied\":true}}");
        }
        Ok::<(), Error>(())
    };
    tokio::select! { result = video => result?, result = inputs => result? }
    session.close().await?;
    runtime.current_environment(device).await?;
    println!("{{\"closed\":true,\"legacy_environment\":true}}");
    Ok(())
}
