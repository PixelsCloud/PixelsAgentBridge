//! Isolated authenticated production Executor + verified native media helper.
//! Requires a smoke data directory; never replaces an installed service or identity.
use pab_agent_core::{DataPaths, DataScope};
use std::{path::PathBuf, time::Duration};
type Error = Box<dyn std::error::Error + Send + Sync>;

fn main() -> Result<(), Error> {
    let raw: Vec<_> = std::env::args_os().skip(1).collect();
    if raw.first().is_some_and(|a| a == "--media-worker") {
        if raw.get(1).is_some_and(|a| a == "--smoke-pattern-proof") {
            let root=PathBuf::from(raw.get(2).ok_or("proof directory required")?);
            if !root.is_absolute() || !root.components().any(|c| c.as_os_str()=="smoke") { return Err("proof directory must be an absolute smoke path".into()); }
            let mut args: Vec<String>=raw.iter().skip(3).map(|a|a.to_string_lossy().into_owned()).collect();
            if args.first().is_some_and(|a| a=="capture") {
                let mode=args.get_mut(3).ok_or("missing capture mode")?;
                if mode.as_str()=="desktop" { *mode="desktop-proof".into(); }
                else if mode.as_str()=="desktop-software" { *mode="desktop-proof-software".into(); }
                else { return Err("proof requires ordinary desktop capture".into()); }
                if args.len()==4 { args.push(String::new()); }
                args.push(root.join(format!("proof-capture-{}.jsonl",std::process::id())).to_string_lossy().into_owned());
            } else if args.first().is_some_and(|a| a=="render") {
                let mode=args.get_mut(4).ok_or("missing render mode")?;
                if mode.as_str()=="stream" { *mode="stream-proof".into(); }
                else if mode.as_str()=="stream-software" { *mode="stream-proof-software".into(); }
                else { return Err("proof requires noninteractive stream renderer".into()); }
            }
            std::process::exit(pab_desktop_media::worker_main(args));
        }
        std::process::exit(pab_desktop_media::worker_main(std::env::args().skip(2)));
    }
    if cfg!(debug_assertions) { return Err("performance host requires cargo build --release".into()); }
    if !raw.iter().any(|a| a == "--config") { return Err("explicit isolated TOML configuration required".into()); }
    let (args, values) = pab_service_config::client::startup(pab_service_config::client::Role::Executor, raw)?;
    for (key, value) in values { unsafe { std::env::set_var(key, value); } }
    let root = DataPaths::for_scope(DataScope::Machine)?.root().to_path_buf();
    if !root.components().any(|c| c.as_os_str() == "smoke") { return Err("host data directory must be under smoke".into()); }
    if args.len() != 3 { return Err("usage: --config FILE host SECONDS IPC_PORT | --config FILE helper PARENT_PID IPC_PORT".into()); }
    let port: u16 = args[2].parse()?;
    if port < 1024 { return Err("isolated unprivileged IPC port required".into()); }
    unsafe { std::env::set_var("PAB_LOCAL_IPC_PORT", port.to_string()); }
    pab_logging::init("smoke-desktop-host", &root)?;
    tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build()?.block_on(async {
        match args[0].as_str() {
            "helper" | "helper-proof" => {
                let parent: u32 = args[1].parse()?;
                let marker = pab_os_control::process_identity(parent)?;
                let mut prefix=vec!["--media-worker".into()];
                if args[0]=="helper-proof" { prefix.extend(["--smoke-pattern-proof".into(),root.to_string_lossy().into_owned()]); }
                let backend = pab_desktop_media::NativeBackend::new(std::env::current_exe()?, prefix);
                let ended = async {
                    loop {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        if pab_os_control::process_identity(parent).ok().as_ref() != Some(&marker) { break; }
                    }
                };
                tokio::select! {
                    _ = ended => {},
                    result = async {
                        loop {
                            if let Err(error) = pab_executor::local_ipc::media::serve_helper(&backend).await {
                                tracing::debug!(%error, "isolated media helper disconnected");
                            }
                            backend.idle().await;
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                        #[allow(unreachable_code)] Ok::<(), Error>(())
                    } => { result?; }
                }
                backend.idle().await;
            }
            "host" | "host-proof" => {
                let seconds: u64 = args[1].parse()?;
                if !(60..=3600).contains(&seconds) { return Err("invalid isolated host duration".into()); }
                let local = pab_executor::local_ipc::spawn_local_service(root.clone());
                let config = pab_executor::bootstrapped_config().await?;
                // The caller must restrict this isolated directory to the test
                // account and SYSTEM. Never print or copy an operator's store.
                let access = pab_executor::read_local_device_status().await?;
                std::fs::write(root.join("access-password.txt"), &access.temporary_password)?;
                std::fs::write(root.join("registration.json"), serde_json::to_vec(&serde_json::json!({
                    "device_id":config.device_id, "device_code":access.device_code,
                    "debug_assertions":cfg!(debug_assertions), "pid":std::process::id()
                }))?)?;
                let config_path = std::env::args_os().collect::<Vec<_>>();
                let at = config_path.iter().position(|a| a == "--config").ok_or("missing configuration")?;
                let mut child = tokio::process::Command::new(std::env::current_exe()?);
                child.args(["--config"]).arg(PathBuf::from(&config_path[at+1]))
                    .args([if args[0]=="host-proof" {"helper-proof"} else {"helper"}, &std::process::id().to_string(), &port.to_string()]).kill_on_drop(true);
                #[cfg(windows)] child.creation_flags(0x08000000);
                let mut helper = child.spawn()?;
                tokio::select! {
                    result = pab_executor::run_executor(config) => { result?; }
                    _ = tokio::time::sleep(Duration::from_secs(seconds)) => {},
                }
                helper.kill().await?;
                helper.wait().await?;
                local.abort();
                std::fs::remove_file(root.join("access-password.txt"))?;
            }
            _ => return Err("unknown isolated host operation".into()),
        }
        Ok(())
    })
}
