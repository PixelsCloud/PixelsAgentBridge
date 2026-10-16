//! Opt-in native acceptance over the product's account/password connection.
//! Uses a fresh ephemeral operator endpoint; never copies an installed private
//! endpoint key, changes a saved password, or replaces the installed app.
use pab_agent_core::DataPaths;
use pab_bridge::{
    BridgeConfig, BridgeError, BridgeRuntime, BridgeRuntimeConfig, RuntimeError,
    SqliteDevicePasswordProvider,
};
use pab_desktop_media::NativeViewer;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
type Error = Box<dyn std::error::Error + Send + Sync>;

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
            // Single threaded, before the runtime or native workers start.
            unsafe {
                std::env::set_var(key, value);
            }
        }
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()?
            .block_on(async { tokio::time::timeout(Duration::from_secs(75), run(args)).await? })
    })();
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("remote desktop acceptance failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
async fn run(args: Vec<String>) -> Result<(), Error> {
    if args.len() != 2 {
        return Err("usage: remote_desktop_check [--config FILE] DEVICE_CODE FPS".into());
    }
    let code = args[0].parse()?;
    let fps: u16 = args[1].parse()?;
    if !matches!(fps, 15 | 30) {
        return Err("FPS must be 15 or 30".into());
    }
    let temp = tempfile::tempdir()?;
    let config = BridgeConfig::register_guest_with_secret(temp.path().join("operator.key")).await?;
    let password_database = DataPaths::for_operator()?.bridge_database();
    let passwords = Arc::new(SqliteDevicePasswordProvider::new(password_database));
    let mut options = BridgeRuntimeConfig::new(temp.path().join("runtime.sqlite3"));
    options.resume_incomplete_on_start = false;
    let runtime = BridgeRuntime::start(config, options, passwords).await?;
    let result = measure(&runtime, code, fps).await;
    runtime.shutdown().await?;
    result
}
async fn measure(
    runtime: &BridgeRuntime,
    code: pab_protocol::DeviceCode,
    fps: u16,
) -> Result<(), Error> {
    let device = runtime.resolve_device_code(code).await?;
    let mut session = runtime.open_remote_desktop(device, None, fps).await?;
    match runtime.open_remote_desktop(device, None, fps).await {
        Err(RuntimeError::Bridge(BridgeError::RemoteDesktop(
            pab_remote_desktop::DesktopError::Rejected {
                code: pab_protocol::DesktopErrorCode::Busy,
                ..
            },
        ))) => {}
        Err(error) => return Err(error.into()),
        Ok(_) => return Err("second viewer was incorrectly accepted".into()),
    }
    let executable: PathBuf = std::env::current_exe()?;
    let measured = u64::from(fps) * 5;
    let warmup = u64::from(fps);
    let mut generations = Vec::new();
    for generation in 1..=2 {
        if generation > 1 {
            session
                .select_screen(session.opened().config.screen.id.clone())
                .await?;
        }
        let config = session.opened().config.clone();
        let mut viewer =
            NativeViewer::open(&executable, &["--media-worker".into()], &config, false).await?;
        let mut presentations = viewer.presentations();
        let acknowledgements = session.presentation_control();
        let output = async {
            loop {
                let header = presentations.next().await?;
                acknowledgements.report_presented(&header)?;
                if header.frame_id == warmup + measured {
                    return Ok::<(), Error>(());
                }
            }
        };
        let submit = async {
            let mut started = Instant::now();
            let mut legacy_max_ms = 0.0f64;
            for index in 0..warmup + measured {
                let frame = session.next_frame(Duration::from_secs(10)).await?;
                viewer.submit(&frame).await?;
                if index % u64::from(fps) == 0 {
                    let query = Instant::now();
                    runtime.current_environment(device).await?;
                    legacy_max_ms = legacy_max_ms.max(query.elapsed().as_secs_f64() * 1000.0);
                }
                if index + 1 == warmup {
                    started = Instant::now();
                }
            }
            let display = viewer.finish().await?;
            Ok::<_, Error>((display, started.elapsed(), legacy_max_ms))
        };
        let ((display, elapsed, legacy_max_ms), ()) = tokio::try_join!(submit, output)?;
        let confirmed = display["displayed"]
            .as_u64()
            .ok_or("missing display statistics")?;
        // Subtract every warmup submission, even if one was not confirmed.
        // This is a conservative count of measured displayed frames, never a
        // count of decoded/submitted packets. Relay jitter may coalesce presents.
        let effective = confirmed.saturating_sub(warmup) as f64 / elapsed.as_secs_f64();
        if confirmed > warmup + measured || effective <= 12.0 || effective < f64::from(fps) * 0.9 {
            return Err(format!("display acceptance failed: {effective:.2} FPS, {display}").into());
        }
        generations.push(serde_json::json!({"generation":config.generation,"screen":config.screen,
            "effective_display_fps":effective,"display":display,
            "unconfirmed_presentations":warmup+measured-confirmed,"parallel_environment_max_ms":legacy_max_ms}));
    }
    let path = format!("{:?}", session.connection_path());
    session.close().await?;
    // A normal environment query on the same runtime must remain usable.
    runtime.current_environment(device).await?;
    println!(
        "{}",
        serde_json::json!({"authenticated_product_connection":true,"device_code":code.to_string(),
        "target_fps":fps,"measured_frames_per_generation":measured,"warmup_frames_per_generation":warmup,
        "connection_path":path,"generations":generations,"same_screen_restart":true,"second_viewer_rejected":true,
        "legacy_environment_after_close":true})
    );
    Ok(())
}
