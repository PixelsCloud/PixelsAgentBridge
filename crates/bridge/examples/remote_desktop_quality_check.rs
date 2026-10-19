//! Opt-in native presentation/quality sampling on an existing product connection.
//! The external driver owns network shaping and deployment/rollback. This probe
//! never changes device configuration, input permissions, or network settings.
use pab_agent_core::DataPaths;
use pab_bridge::{BridgeConfig, BridgeRuntime, BridgeRuntimeConfig, SqliteDevicePasswordProvider};
use pab_desktop_media::NativeViewer;
use std::{
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
            unsafe {
                std::env::set_var(key, value);
            }
        }
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()?
            .block_on(async { tokio::time::timeout(Duration::from_secs(150), run(args)).await? })
    })();
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("quality acceptance failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(args: Vec<String>) -> Result<(), Error> {
    if args.len() != 1 {
        return Err("usage: remote_desktop_quality_check [--config FILE] DEVICE_CODE".into());
    }
    let temporary = tempfile::tempdir()?;
    let config =
        BridgeConfig::register_guest_with_secret(temporary.path().join("operator.key")).await?;
    let passwords = Arc::new(SqliteDevicePasswordProvider::new(
        DataPaths::for_operator()?.bridge_database(),
    ));
    let mut options = BridgeRuntimeConfig::new(temporary.path().join("runtime.sqlite3"));
    options.resume_incomplete_on_start = false;
    let runtime = BridgeRuntime::start(config, options, passwords).await?;
    let result = measure(&runtime, args[0].parse()?).await;
    runtime.shutdown().await?;
    result
}

async fn measure(runtime: &BridgeRuntime, code: pab_protocol::DeviceCode) -> Result<(), Error> {
    let device = runtime.resolve_device_code(code).await?;
    let mut session = runtime.open_remote_desktop(device, None, 15).await?;
    let executable = std::env::current_exe()?;
    let started = Instant::now();
    for seconds in [35, 65] {
        let config = session.opened().config.clone();
        let mut viewer =
            NativeViewer::open(&executable, &["--media-worker".into()], &config, false).await?;
        let mut presentations = viewer.presentations();
        let acknowledgements = session.presentation_control();
        let mut quality = session.quality_control();
        println!(
            "{}",
            serde_json::json!({"event":"generation_started", "elapsed_ms":started.elapsed().as_millis(),"config":config,"path":format!("{:?}",session.connection_path())})
        );
        let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
        let (sent, mut final_frame) = tokio::sync::watch::channel(None::<u64>);
        let display = async {
            let mut presented = 0;
            loop {
                if final_frame.borrow().is_some_and(|last| presented >= last) {
                    return Ok::<(), Error>(());
                }
                tokio::select! {
                    header = presentations.next() => {
                        let header = header?;
                        acknowledgements.report_presented(&header)?;
                        presented = header.frame_id;
                        println!("{}",serde_json::json!({"event":"presented","elapsed_ms":started.elapsed().as_millis(),"generation":header.generation,"frame":header.frame_id}));
                    },
                    result = final_frame.changed() => { result?; },
                }
            }
        };
        let diagnostics = async {
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            loop {
                interval.tick().await;
                let snapshot = quality.snapshot().await?;
                let query = Instant::now();
                runtime.current_environment(device).await?;
                println!(
                    "{}",
                    serde_json::json!({"event":"quality","elapsed_ms":started.elapsed().as_millis(),"snapshot":snapshot,"parallel_environment_ms":query.elapsed().as_secs_f64()*1000.0})
                );
            }
            #[allow(unreachable_code)]
            Ok::<(), Error>(())
        };
        let receive = async {
            let mut last = 0;
            loop {
                let frame = session.next_frame(Duration::from_secs(10)).await?;
                println!(
                    "{}",
                    serde_json::json!({"event":"received","elapsed_ms":started.elapsed().as_millis(),"generation":frame.header.generation,"frame":frame.header.frame_id,"bytes":frame.bytes.len(),"capture_time_us":frame.header.capture_time_us})
                );
                viewer.submit(&frame).await?;
                last = frame.header.frame_id;
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
            }
            sent.send_replace(Some(last));
            viewer.finish().await.map_err(Error::from)
        };
        let statistics = tokio::select! {
            value = async { tokio::try_join!(receive, display) } => value?.0,
            value = diagnostics => { value?; return Err("quality diagnostics stopped".into()); },
        };
        println!(
            "{}",
            serde_json::json!({"event":"generation_finished","elapsed_ms":started.elapsed().as_millis(),"generation":config.generation,"statistics":statistics})
        );
        if seconds == 35 {
            session.select_screen(config.screen.id).await?;
        }
    }
    session.close().await?;
    runtime.current_environment(device).await?;
    println!(
        "{}",
        serde_json::json!({"event":"closed","legacy_environment_after_close":true})
    );
    Ok(())
}
