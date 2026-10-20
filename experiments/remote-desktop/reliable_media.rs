//! Explicit, short-lived experiment. Production still uses DeviceSessionAcceptor
//! and its existing password/account auth. Separate experiment endpoints pin the
//! peer's ephemeral QUIC public key supplied over the already-authorized channel.
mod media_child;
use media_child::{NativeBackend, NativeViewer};
use pab_protocol::*;
use pab_remote_desktop::*;
use pab_transport::*;
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
const WAIT: Duration = Duration::from_secs(10);
type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Serialize, Deserialize)]
struct Offer {
    endpoint_key: [u8; 32],
    direct_addresses: Vec<std::net::SocketAddr>,
}
async fn endpoint(relay: &str) -> std::result::Result<PabEndpoint, Error> {
    Ok(PabEndpoint::bind(
        PabEndpointConfig::new(vec![relay.parse()?])?,
        iroh_base::SecretKey::generate(),
    )
    .await?)
}
async fn offer(endpoint: &PabEndpoint) -> std::result::Result<Offer, Error> {
    let mut watch = endpoint.watch_address();
    tokio::time::timeout(WAIT, async {
        while watch.borrow().direct_addresses.is_empty() {
            watch.changed().await?;
        }
        Ok::<(), Error>(())
    })
    .await??;
    Ok(Offer {
        endpoint_key: *endpoint.id().as_bytes(),
        direct_addresses: watch.borrow().direct_addresses.clone(),
    })
}
fn print(value: &impl Serialize) -> std::result::Result<(), Error> {
    println!("{}", serde_json::to_string(value)?);
    std::io::stdout().flush()?;
    Ok(())
}
async fn serve(
    connection: PabConnection,
    backend: Arc<NativeBackend>,
    peer: [u8; 32],
) -> std::result::Result<(), Error> {
    if connection.remote_endpoint_key() != peer {
        return Err("unapproved experiment peer".into());
    }
    let host = DesktopHost::new(Some(backend.clone())).for_connection();
    let guard = host.connection_guard();
    let actor = OperatorRef::guest(EndpointKey::new(peer));
    let mut streams = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            value = connection.accept_bi(WAIT) => match value {
                Ok(mut stream) => {
                    let host = host.clone();
                    streams.spawn(async move {
                        match stream.receive_json::<DeviceTaskRequest>(WAIT).await? {
                            DeviceTaskRequest::RemoteDesktop { schema_version:DEVICE_TASK_SCHEMA_VERSION, request } => host.serve(actor,None,request,stream,WAIT).await?,
                            DeviceTaskRequest::GetPresence { schema_version:DEVICE_TASK_SCHEMA_VERSION } => stream.send_json(&DeviceTaskResponse::Presence {active_operators:0},WAIT).await?,
                            _ => return Err::<(),Error>("unexpected experiment request".into()),
                        }
                        Ok::<(),Error>(())
                    });
                }
                Err(PabConnectionError::Timeout) => continue,
                Err(_) => break,
            },
            _ = streams.join_next(), if !streams.is_empty() => {},
        }
    }
    drop(guard);
    streams.abort_all();
    while streams.join_next().await.is_some() {}
    tokio::time::timeout(WAIT, backend.idle()).await?;
    Ok(())
}
async fn legacy(connection: &PabConnection) -> std::result::Result<f64, Error> {
    let start = Instant::now();
    let mut stream = connection.open_bi(WAIT).await?;
    stream
        .send_frame_json(
            &DeviceTaskRequest::GetPresence {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
            },
            WAIT,
        )
        .await?;
    if !matches!(
        stream.receive_json(WAIT).await?,
        DeviceTaskResponse::Presence { .. }
    ) {
        return Err("legacy response changed".into());
    }
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}
async fn view(
    connection: &PabConnection,
    helper: PathBuf,
    fps: u16,
    frames: u64,
    verify: bool,
    restart: bool,
) -> std::result::Result<serde_json::Value, Error> {
    let mut session = RemoteDesktopSession::open(connection.clone(), None, fps, WAIT).await?;
    let mut results = Vec::new();
    for round in 0..if restart { 2 } else { 1 } {
        if round > 0 {
            session
                .select_screen(session.opened().config.screen.id.clone())
                .await?;
        }
        let config = session.opened().config.clone();
        let mut renderer =
            tokio::time::timeout(WAIT, NativeViewer::open(&helper, &config, verify)).await??;
        let mut render_ms = Vec::new();
        let mut bytes = 0;
        let mut max_legacy_ms: f64 = 0.0;
        let warmup = u64::from(fps);
        let mut start = Instant::now();
        for index in 0..frames + warmup {
            let frame = session.next_frame(WAIT).await?;
            let decode = Instant::now();
            tokio::time::timeout(WAIT, renderer.present(&frame)).await??;
            if index >= warmup {
                bytes += frame.bytes.len();
                render_ms.push(decode.elapsed().as_secs_f64() * 1000.0);
            }
            session.report_presented(&frame.header)?;
            if index % u64::from(fps) == 0 {
                max_legacy_ms = max_legacy_ms.max(legacy(connection).await?);
            }
            if index + 1 == warmup {
                start = Instant::now();
            }
        }
        let displayed = tokio::time::timeout(WAIT, renderer.finish()).await??;
        let seconds = start.elapsed().as_secs_f64();
        render_ms.sort_by(f64::total_cmp);
        results.push(serde_json::json!({"generation":config.generation,"screen":config.screen,"frames":frames,"warmup_frames":warmup,
            "display":displayed,"seconds":seconds,"effective_display_fps":frames as f64/seconds,"encoded_bytes":bytes,
            "receive_to_renderer_consumed_p95_ms":render_ms[(render_ms.len()-1)*95/100],"parallel_legacy_max_ms":max_legacy_ms}));
    }
    session.close().await?;
    legacy(connection).await?;
    Ok(
        serde_json::json!({"stage":"reliable_native_media","path":format!("{:?}",connection.selected_path()),"target_fps":fps,
        "cases":results,"media_pixel_cpu_readbacks":0,"media_gpu_texture_copies":0,
        "display_confirmation":"dxgi_frame_statistics_each_present_id","installed_application_modified":false}),
    )
}
async fn run() -> std::result::Result<(), Error> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        return Err("usage: reliable-media loopback|client HELPER FPS FRAMES synthetic|animate|desktop verify|normal restart|once [RELAY]; server HELPER SOURCE CLIENT_KEY_JSON [RELAY]".into());
    }
    let helper = std::fs::canonicalize(&args[2])?;
    if args[1] == "server" {
        if !(5..=6).contains(&args.len()) {
            return Err("invalid server arguments".into());
        }
        let peer: [u8; 32] = serde_json::from_str(&args[4])?;
        let endpoint = endpoint(
            args.get(5)
                .map(String::as_str)
                .unwrap_or("https://127.0.0.1:1"),
        )
        .await?;
        let backend = Arc::new(NativeBackend::new(helper, args[3].clone()));
        backend.screens().await?;
        print(&offer(&endpoint).await?)?;
        loop {
            let connection = endpoint
                .accept()
                .await?
                .ok_or("experiment endpoint closed")?;
            if connection.remote_endpoint_key() != peer {
                connection.close(b"unapproved peer");
                continue;
            }
            serve(connection, backend, peer).await?;
            break;
        }
        return Ok(());
    }
    if !(8..=9).contains(&args.len()) {
        return Err("invalid viewer arguments".into());
    }
    let fps: u16 = args[3].parse()?;
    let frames: u64 = args[4].parse()?;
    if !(1..=60).contains(&fps) || frames == 0 || frames > 1800 {
        return Err("invalid experiment duration/FPS".into());
    }
    if !["synthetic", "animate", "desktop"].contains(&args[5].as_str())
        || !["verify", "normal"].contains(&args[6].as_str())
        || !["restart", "once"].contains(&args[7].as_str())
    {
        return Err("invalid mode".into());
    }
    let verify = args[6] == "verify";
    if verify && args[5] != "synthetic" {
        return Err("real desktop pixel readback refused".into());
    }
    let relay = args
        .get(8)
        .map(String::as_str)
        .unwrap_or("https://127.0.0.1:1");
    let local = endpoint(relay).await?;
    let mut serving = None;
    let mut remote_endpoint = None;
    let address = if args[1] == "loopback" {
        let remote = endpoint(relay).await?;
        let address = offer(&remote).await?;
        let peer = *local.id().as_bytes();
        let endpoint = remote.clone();
        let backend = Arc::new(NativeBackend::new(helper.clone(), args[5].clone()));
        serving = Some(tokio::spawn(async move {
            let connection = endpoint
                .accept()
                .await?
                .ok_or("experiment endpoint closed")?;
            serve(connection, backend, peer).await
        }));
        remote_endpoint = Some(remote);
        address
    } else if args[1] == "client" {
        print(&serde_json::json!({"endpoint_key":local.id().as_bytes()}))?;
        // Tokio's blocking stdin reader can keep runtime shutdown waiting after
        // timeout. This dedicated thread owns no child/resources and does not
        // prevent the short-lived experiment process from exiting on its deadline.
        let (send, receive) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            use std::io::{BufRead, Read};
            let mut line = String::new();
            let result = std::io::stdin()
                .lock()
                .take(65537)
                .read_line(&mut line)
                .map(|_| line);
            let _ = send.send(result);
        });
        let line = receive.await??;
        if line.len() > 65536 {
            return Err("offer too large".into());
        }
        serde_json::from_str(&line)?
    } else {
        return Err("unknown experiment mode".into());
    };
    let connection = local
        .connect(
            address.endpoint_key,
            &PabEndpointAddress {
                direct_addresses: address.direct_addresses,
                relay_urls: vec![relay.into()],
            },
            WAIT,
        )
        .await?;
    let result = view(
        &connection,
        helper,
        fps,
        frames,
        verify,
        args[7] == "restart",
    )
    .await;
    connection.close(b"media experiment complete");
    if let Some(task) = serving {
        tokio::time::timeout(WAIT, task).await???;
    }
    drop(remote_endpoint);
    print(&result?)?;
    Ok(())
}
#[tokio::main(worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    match tokio::time::timeout(Duration::from_secs(80), run()).await {
        Ok(Ok(())) => std::process::ExitCode::SUCCESS,
        Ok(Err(error)) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
        Err(_) => {
            eprintln!("media experiment expired");
            std::process::ExitCode::FAILURE
        }
    }
}
