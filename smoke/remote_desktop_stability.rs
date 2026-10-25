//! Real authenticated desktop video through the production native renderer.
//! Launch synthetic moving content on the host first. Only frame metadata is
//! written; this probe never records desktop pixels, input or credentials.
use pab_agent_core::DataPaths;
use pab_bridge::{
    BridgeConfig, BridgeRuntime, BridgeRuntimeConfig, DevicePasswordProvider,
    FileDevicePasswordProvider, SqliteDevicePasswordProvider,
};
use pab_desktop_media::NativeViewer;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
type Error = Box<dyn std::error::Error + Send + Sync>;
mod remote_desktop_telemetry;
mod remote_desktop_network;
use remote_desktop_telemetry::{Telemetry, emit};
fn main() -> std::process::ExitCode {
    let telemetry = Telemetry::start();
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
            .block_on(run(args))
    })();
    let status = match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            emit(serde_json::json!({"event":"error","message":error.to_string()}));
            std::process::ExitCode::FAILURE
        }
    };
    match telemetry.finish() {
        Ok(0) => status,
        result => {
            eprintln!("measurement output incomplete: {result:?}");
            std::process::ExitCode::FAILURE
        }
    }
}
async fn run(mut args: Vec<String>) -> Result<(), Error> {
    let probe_peer = if let Some(at) = args.iter().position(|a| a == "--network-probe") {
        if at+1 >= args.len() { return Err("--network-probe requires private IPv4:port".into()); }
        let peer = remote_desktop_network::parse_peer(&args.remove(at+1))?;
        args.remove(at);
        Some(peer)
    } else { None };
    let archive = if let Some(at)=args.iter().position(|a|a=="--proof-packets") {
        if at+1>=args.len() { return Err("--proof-packets requires a path".into()); }
        let path=PathBuf::from(args.remove(at+1)); args.remove(at);
        if !path.is_absolute() || !path.components().any(|c|c.as_os_str()=="smoke") { return Err("packet archive must be an absolute smoke path".into()); }
        Some(path)
    } else {None};
    let proof=args.iter().any(|a|a=="--pattern-proof");
    let software=args.iter().any(|a|a=="--software-decoder");
    let relay=args.iter().any(|a|a=="--relay");
    if archive.is_some() && !proof { return Err("packet archives require explicit synthetic pattern proof mode".into()); }
    args.retain(|a|!matches!(a.as_str(),"--pattern-proof"|"--software-decoder"|"--relay"));
    if cfg!(debug_assertions) { return Err("stability measurements require cargo build --release".into()); }
    if !(4..=5).contains(&args.len()) {
        return Err(
        "usage: remote_desktop_stability [--config FILE] DEVICE_CODE FPS SECONDS INSTALLED_DESKTOP [PASSWORD_FILE]".into());
    }
    let code = args[0].parse()?;
    let fps: u16 = args[1].parse()?;
    let seconds: u64 = args[2].parse()?;
    if !matches!(fps, 15 | 30) || !(30..=3600).contains(&seconds) {
        return Err("invalid FPS or duration".into());
    }
    let worker = PathBuf::from(&args[3]);
    let temp = tempfile::tempdir()?;
    let config = tokio::time::timeout(
        Duration::from_secs(40),
        BridgeConfig::register_guest_with_secret(temp.path().join("operator.key")),
    )
    .await??;
    let passwords: Arc<dyn DevicePasswordProvider> = if let Some(path) = args.get(4) {
        Arc::new(FileDevicePasswordProvider::new(path))
    } else {
        Arc::new(SqliteDevicePasswordProvider::new(
            DataPaths::for_operator()?.bridge_database(),
        ))
    };
    let mut options = BridgeRuntimeConfig::new(temp.path().join("runtime.sqlite3"));
    options.resume_incomplete_on_start = false;
    let runtime = BridgeRuntime::start(config, options, passwords).await?;
    let result = tokio::time::timeout(Duration::from_secs(seconds + 70), async {
        let device = runtime.resolve_device_code(code).await?;
        if relay { runtime.connect_device_with_mode(device,Some(pab_bridge::ConnectionMode::Relay)).await?; }
        let mut session = runtime.open_remote_desktop(device, None, fps).await?;
        let config = session.opened().config.clone();
        emit(serde_json::json!({"event":"opened","device_code":args[0],"config":config,
            "seconds":seconds,"worker":worker,"debug_assertions":cfg!(debug_assertions),
            "pattern_proof":proof,"software_decoder_requested":software,"relay_requested":relay,
            "path":format!("{:?}",session.connection_path())}));
        let mut prefix=vec!["--media-worker".into()];
        if proof { prefix.extend(["--smoke-pattern-proof".into(),worker.parent().ok_or("worker directory required")?.to_string_lossy().into_owned()]); }
        let mut viewer = if software { NativeViewer::open_software(&worker,&prefix,&config,false).await? }
            else { NativeViewer::open(&worker,&prefix,&config,false).await? };
        let mut archive=archive.as_ref().map(|path|std::fs::OpenOptions::new().write(true).create_new(true).open(path)).transpose()?;
        if let Some(file)=archive.as_mut() {
            use std::io::Write;
            file.write_all(b"PABPROOF1")?;
            file.write_all(&config.screen.pixel_width.to_le_bytes())?;
            file.write_all(&config.screen.pixel_height.to_le_bytes())?;
            file.write_all(&u32::from(config.fps).to_le_bytes())?;
        }
        let mut archive_bytes=21usize;
        let mut presentations = viewer.presentations();
        let acknowledgements = session.presentation_control();
        let mut quality = session.quality_control();
        let mut remote = quality.clone();
        let network = quality.clone();
        let started = Instant::now();
        let _network_probe = probe_peer.map(|peer| remote_desktop_network::Monitor::start(
            peer, started, Duration::from_secs(seconds))).transpose()?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
        let counters = Mutex::new(pab_remote_desktop::ViewerDiagnostics::new(started));
        let (last_tx, mut last_rx) = tokio::sync::watch::channel(None::<u64>);
        let display = async {
            let mut last = 0;
            let mut confirmations = 0;
            loop {
                if last_rx.borrow().is_some_and(|id| last >= id) { break; }
                tokio::select! {
                    header = presentations.next() => {
                        let header = header?; let now = Instant::now();
                        acknowledgements.report_presented(&header)?;
                        counters.lock().unwrap().presented(header.frame_id, now);
                        last = header.frame_id; confirmations += 1;
                        emit(serde_json::json!({"event":"presented","elapsed_us":started.elapsed().as_micros(),
                            "frame":last,"decoder":presentations.decoder(),"native_timings":presentations.timings()}));
                    },
                    changed = last_rx.changed() => { changed?; }
                }
            }
            Ok::<_, Error>(confirmations)
        };
        let receive = async {
            let mut last = 0;
            loop {
                // Cancellation is only used at final shutdown; no partially
                // read frame is ever reused for another read or generation.
                let reading = Instant::now();
                let frame = tokio::select! {
                    result = session.next_frame_when_changed() => result?,
                    _ = tokio::time::sleep_until(deadline) => {
                        last_tx.send_replace(Some(last)); break;
                    }
                };
                let now = Instant::now(); let id = frame.header.frame_id;
                if let Some(file)=archive.as_mut() {
                    use std::io::Write;
                    archive_bytes=archive_bytes.checked_add(12+frame.bytes.len()).ok_or("proof archive size overflow")?;
                    if archive_bytes>64*1024*1024 { return Err("synthetic packet archive reached its 64 MiB bound".into()); }
                    file.write_all(&(frame.bytes.len() as u32).to_le_bytes())?;
                    file.write_all(&(id-1).to_le_bytes())?;
                    file.write_all(&frame.bytes)?;
                    file.flush()?;
                }
                last = id;
                counters.lock().unwrap().received(id, frame.bytes.len(), now);
                emit(serde_json::json!({"event":"received","elapsed_us":started.elapsed().as_micros(),
                    "frame":id,"bytes":frame.bytes.len(),"capture_time_us":frame.header.capture_time_us,
                    "fingerprint":if proof {Some(packet_fingerprint(&frame.bytes))}else{None},"keyframe":frame.header.keyframe,
                    "read_us":reading.elapsed().as_micros()}));
                let submit = Instant::now(); viewer.submit(&frame).await?;
                counters.lock().unwrap().submitted(id, submit.elapsed());
                emit(serde_json::json!({"event":"submitted","elapsed_us":started.elapsed().as_micros(),
                    "frame":id,"submit_us":submit.elapsed().as_micros()}));
                if started.elapsed() >= Duration::from_secs(seconds) { last_tx.send_replace(Some(id)); break; }
            }
            viewer.finish().await
        };
        let diagnostics = async {
            let mut timer = tokio::time::interval(Duration::from_secs(2));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                timer.tick().await;
                let local = counters.lock().unwrap().snapshot(Instant::now());
                let host = remote.diagnostics().await;
                let budget = quality.snapshot().await;
                emit(serde_json::json!({"event":"diagnostics","elapsed_us":started.elapsed().as_micros(),
                    "local":local,"remote":host.as_ref().ok(),"remote_error":host.err().map(|e|e.to_string()),
                    "quality":budget.as_ref().ok(),"quality_error":budget.err().map(|e|e.to_string()),
                    "rtt_ms":network.network_rtt().map(|r|r.as_secs_f64()*1000.0),
                    "path":format!("{:?}",network.connection_path())}));
            }
            #[allow(unreachable_code)] Ok::<(), Error>(())
        };
        let traces = async {
            let mut remote_trace = network.clone();
            let mut cursor = 0;
            let mut timer = tokio::time::interval(Duration::from_millis(500));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                timer.tick().await;
                let request = Instant::now();
                let trace = remote_trace.frame_trace(cursor).await;
                if let Ok(snapshot) = &trace { cursor = snapshot.latest_frame_id; }
                emit(serde_json::json!({"event":"frame_trace","elapsed_us":started.elapsed().as_micros(),
                    "query_us":request.elapsed().as_micros(),"trace":trace.as_ref().ok(),
                    "error":trace.err().map(|e|e.to_string()),"receiver_network":network.network_statistics(),
                    "receiver_control":acknowledgements.trace()}));
            }
            #[allow(unreachable_code)] Ok::<(), Error>(())
        };
        let runtime_probe = async {
            let mut at = tokio::time::Instant::now() + Duration::from_millis(50);
            loop {
                tokio::time::sleep_until(at).await;
                let now = tokio::time::Instant::now();
                emit(serde_json::json!({"event":"runtime_probe","elapsed_us":started.elapsed().as_micros(),
                    "late_us":now.saturating_duration_since(at).as_micros()}));
                at = (at + Duration::from_millis(50)).max(now + Duration::from_millis(1));
            }
            #[allow(unreachable_code)] Ok::<(), Error>(())
        };
        let (summary, confirmations) = tokio::select! {
            value = async { tokio::try_join!(receive, display) } => value?,
            value = diagnostics => { value?; return Err("diagnostics unexpectedly ended".into()); }
            value = traces => { value?; return Err("frame trace unexpectedly ended".into()); }
            value = runtime_probe, if probe_peer.is_some() => { value?; return Err("runtime probe unexpectedly ended".into()); }
        };
        emit(serde_json::json!({"event":"control_trace","elapsed_us":started.elapsed().as_micros(),
            "receiver_control":acknowledgements.trace()}));
        let mut final_trace = network.clone();
        let trace = final_trace.frame_trace(0).await;
        emit(serde_json::json!({"event":"frame_trace","elapsed_us":started.elapsed().as_micros(),
            "final":true,"trace":trace.as_ref().ok(),"error":trace.err().map(|e|e.to_string()),
            "receiver_network":network.network_statistics()}));
        session.close().await?;
        emit(serde_json::json!({"event":"closed","elapsed_us":started.elapsed().as_micros(),
            "native_summary":summary,"presentation_notifications":confirmations}));
        Ok::<(), Error>(())
    }).await;
    runtime.shutdown().await?;
    result?
}

fn packet_fingerprint(data:&[u8])->String {
    let hash=data.iter().fold(14695981039346656037u64,|hash,b|(hash^u64::from(*b)).wrapping_mul(1099511628211));
    format!("{hash:x}")
}
