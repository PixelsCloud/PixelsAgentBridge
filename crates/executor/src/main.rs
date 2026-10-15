use std::{path::Path, process::ExitCode};

use pab_agent_core::{DataPaths, DataScope};
use pab_executor::{bootstrapped_config, rotate_temporary_password, run_executor, show_access};

#[cfg(windows)]
mod windows_service;

fn main() -> ExitCode {
    let raw: Vec<_> = std::env::args_os().skip(1).collect();
    if raw.first().is_some_and(|a| a == "configure") {
        let args: Result<Vec<_>, _> = raw.into_iter().skip(1).map(|a| a.into_string()).collect();
        return match args
            .map_err(|_| "Invalid configuration argument".to_owned())
            .and_then(|args| pab_service_config::client::configure(&args))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("pab-executor: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let worker = raw
        .first()
        .is_some_and(|a| a == "--user-worker" || a == "--file-access-probe");
    let args = if worker {
        std::env::args().skip(1).collect()
    } else {
        match pab_service_config::client::startup(pab_service_config::client::Role::Executor, raw) {
            Ok((args, values)) => {
                // Main is still single-threaded, before logging, Tokio or the service dispatcher.
                for (key, value) in values {
                    unsafe {
                        std::env::set_var(key, value);
                    }
                }
                args
            }
            Err(error) => {
                eprintln!("pab-executor: {error}");
                return ExitCode::FAILURE;
            }
        }
    };
    #[cfg(windows)]
    if args.first().is_some_and(|a| a == "--service") {
        let result = DataPaths::for_scope(DataScope::Machine)
            .map_err(|e| e.to_string())
            .and_then(|paths| {
                pab_logging::init("executor", paths.root()).map_err(|e| e.to_string())
            })
            .and_then(|_| windows_service::run());
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("pab-executor: {error}");
                ExitCode::FAILURE
            }
        };
    }
    match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(run_main(args)),
        Err(error) => {
            eprintln!("pab-executor: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run_main(args: Vec<String>) -> ExitCode {
    #[cfg(target_os = "macos")]
    if args.first().map(String::as_str) == Some("--file-access-probe") {
        return if pab_executor::file_access::worker_main().is_ok() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    if args.first().map(String::as_str) == Some("--user-worker") {
        let args = &args[1..];
        if args.len() != 2 {
            return ExitCode::FAILURE;
        }
        let Ok(parent) = args[1].parse() else {
            return ExitCode::FAILURE;
        };
        return match pab_executor::user_worker::run(&args[0], parent).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                let _ = pab_os_control::execution::stop_disconnected_worker_group();
                eprintln!("user worker: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let log_root = match DataPaths::for_scope(DataScope::Machine) {
        Ok(paths) => paths.root().to_path_buf(),
        Err(error) => {
            eprintln!("pab-executor: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = pab_logging::init("executor", &log_root) {
        eprintln!("pab-executor: {error}");
        return ExitCode::FAILURE;
    }
    if args.first().map(String::as_str) == Some("rotate-password") {
        let mut args = args.iter().skip(1);
        let (database, None) = (args.next(), args.next()) else {
            eprintln!("usage: pab-executor rotate-password [device-database]");
            return ExitCode::FAILURE;
        };
        let database = database
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| log_root.join("executor.sqlite3"));
        return match rotate_temporary_password(Path::new(&database)).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(%error, "rotate-password failed");
                eprintln!("pab-executor: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if args.first().map(String::as_str) == Some("issue-local-access") {
        let Some(destination) = args.get(1) else {
            eprintln!("usage: pab-executor issue-local-access <user-token-file>");
            return ExitCode::FAILURE;
        };
        return match pab_executor::local_ipc::issue_local_access(std::path::Path::new(&destination))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(%error, "could not issue local access");
                eprintln!("pab-executor: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if args.first().map(String::as_str) == Some("show-access") {
        return match show_access().await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(%error, "show-access failed");
                eprintln!("pab-executor: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if !args.is_empty() {
        eprintln!("unknown executor command");
        return ExitCode::FAILURE;
    }
    let local_service = pab_executor::local_ipc::spawn_local_service(log_root);
    let result = match bootstrapped_config().await {
        Ok(config) => run_executor(config)
            .await
            .map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    };
    local_service.abort();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "executor failed");
            eprintln!("pab-executor: {error}");
            ExitCode::FAILURE
        }
    }
}
