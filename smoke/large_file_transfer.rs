//! Explicit large-file acceptance using the real Bridge transfer implementation.
//! Reads credentials through the normal provider; never copies or prints them.
use pab_agent_core::DataPaths;
use pab_bridge::{BridgeConfig, BridgeRuntime, BridgeRuntimeConfig, SqliteDevicePasswordProvider};
use pab_protocol::RequestId;
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::io::AsyncReadExt;
type Error = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main]
pub async fn main() -> Result<(), Error> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let init_only = args.len() == 5 && args[4] == "--init-only";
    if (!init_only && args.len() != 4) || args[0] != "428358431" {
        return Err("Usage: large_file_transfer 428358431 REMOTE_SOURCE REMOTE_DESTINATION LOCAL_OUTPUT_DIRECTORY".into());
    }
    let output = PathBuf::from(&args[3]);
    std::fs::create_dir_all(&output)?;
    pab_logging::init("large-transfer", &output)?;
    let mut config =
        BridgeConfig::register_guest_with_secret(output.join("guest-endpoint.key")).await?;
    config.operation_timeout = Duration::from_secs(60);
    let provider = Arc::new(SqliteDevicePasswordProvider::new(
        DataPaths::for_operator()?.bridge_database(),
    ));
    let mut runtime_config = BridgeRuntimeConfig::new(output.join("runtime.sqlite3"));
    runtime_config.resume_incomplete_on_start = false;
    let client = BridgeRuntime::start(config, runtime_config, provider).await?;
    let result = async {
        let device = client.resolve_device_code(args[0].parse()?).await?;
        let context = client.connect_device(device).await?;
        std::fs::write(output.join("target.json"), serde_json::to_vec_pretty(&context)?)?;
        println!("connected: {}", context.compact_reminder());
        if init_only {
            let connection = client.file_connection(device, false).await?;
            let id = RequestId::new();
            let control = pab_bridge::TransferControl::default();
            let ready = tokio::sync::Notify::new();
            let elapsed = Mutex::new(None::<f64>);
            let started = Instant::now();
            let path = output.join("initialization-only.bin");
            let transfer = connection.download_file_controlled(id, &args[1], &path, false,
                &control, |_, size| {
                    let mut elapsed = elapsed.lock().unwrap();
                    if elapsed.is_none() {
                        *elapsed = Some(started.elapsed().as_secs_f64());
                        println!("ready: size={size}, {:.2}s", started.elapsed().as_secs_f64());
                        ready.notify_one();
                    }
                });
            let stop = async {
                ready.notified().await;
                connection.stop_transfer(id, pab_protocol::FileTransferIntent::Cancel).await?;
                control.cancel();
                Ok::<_,pab_bridge::BridgeError>(())
            };
            let (result, stopped) = tokio::time::timeout(Duration::from_secs(600), async {
                tokio::join!(transfer, stop)
            }).await?;
            stopped?;
            let mut snapshot = connection.get_transfer(id).await?;
            for _ in 0..100 {
                if snapshot.state == "cancelled" && snapshot.finished_at_unix_ms.is_some() {break;}
                tokio::time::sleep(Duration::from_millis(100)).await;
                snapshot = connection.get_transfer(id).await?;
            }
            let report = serde_json::json!({"request_id":id,"ready_seconds":*elapsed.lock().unwrap(),
                "sha256":control.digest(),"transfer_result":result.err().map(|e|e.to_string()),
                "snapshot":snapshot,"scope":"initialization only; intentionally cancelled after FileReady"});
            std::fs::write(output.join("initialization-result.json"), serde_json::to_vec_pretty(&report)?)?;
            assert_eq!(snapshot.state,"cancelled");
            return Ok(());
        }
        let path = output.join("candidate-download-8g.bin");
        let size = 8 * 1024_u64.pow(3) + 17;
        let mut report = serde_json::json!({"device":args[0],"size":size,"source":args[1],"destination":args[2],"status":"running"});
        for upload in [false, true] {
            let direction = if upload {"upload"} else {"download"};
            let id = RequestId::new();
            let started = Instant::now();
            let printed = Mutex::new(Instant::now() - Duration::from_secs(30));
            let first = Mutex::new(None::<f64>);
            report[direction] = serde_json::json!({"request_id":id,"state":"running"});
            std::fs::write(output.join("candidate-result.json"), serde_json::to_vec_pretty(&report)?)?;
            let progress = |offset, total| {
                let mut first = first.lock().unwrap();
                if first.is_none() { *first = Some(started.elapsed().as_secs_f64()); }
                let mut printed = printed.lock().unwrap();
                if printed.elapsed() >= Duration::from_secs(30) || offset == total {
                    println!("{direction}: {offset}/{total}, {:.1}s", started.elapsed().as_secs_f64());
                    *printed = Instant::now();
                }
            };
            let result = if upload {
                client.upload_file_with_id(id, device, &path, &args[2], false, progress).await
            } else {
                client.download_file_with_id(id, device, &args[1], &path, false, progress).await
            };
            report[direction] = serde_json::json!({"request_id":id,"state":if result.is_ok(){"completed"}else{"failed"},"seconds":started.elapsed().as_secs_f64(),"ready_seconds":*first.lock().unwrap(),"error":result.as_ref().err().map(ToString::to_string)});
            std::fs::write(output.join("candidate-result.json"), serde_json::to_vec_pretty(&report)?)?;
            result?;
            if !upload {
                assert_eq!(tokio::fs::metadata(&path).await?.len(), size);
                let mut file = tokio::fs::File::open(&path).await?;
                let mut hash = Sha256::new();
                let mut buffer = vec![0;1024*1024];
                loop { let count=file.read(&mut buffer).await?; if count==0 {break;} hash.update(&buffer[..count]); }
                report["local_sha256"] = serde_json::json!(format!("{:x}",hash.finalize()));
            }
        }
        report["status"] = serde_json::json!("completed_pending_independent_remote_hash");
        std::fs::write(output.join("candidate-result.json"), serde_json::to_vec_pretty(&report)?)?;
        Ok::<_,Error>(())
    }.await;
    client.shutdown().await?;
    result
}
