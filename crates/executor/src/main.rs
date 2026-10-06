use std::{path::Path, process::ExitCode};

use pab_agent_core::{DataPaths, DataScope};
use pab_executor::{bootstrapped_config, rotate_temporary_password, run_executor, show_access};

#[cfg(windows)]
mod windows_service;

#[tokio::main]
async fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("--user-worker") {
        let args = std::env::args().skip(2).collect::<Vec<_>>();
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
    #[cfg(windows)]
    if std::env::args().nth(1).as_deref() == Some("--service") {
        return match windows_service::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(%error, "Windows Executor service failed");
                ExitCode::FAILURE
            }
        };
    }
    if std::env::args().nth(1).as_deref() == Some("rotate-password") {
        let mut args = std::env::args().skip(2);
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
    if std::env::args().nth(1).as_deref() == Some("issue-local-access") {
        let Some(destination) = std::env::args().nth(2) else {
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
    if std::env::args().nth(1).as_deref() == Some("show-access") {
        return match show_access().await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(%error, "show-access failed");
                eprintln!("pab-executor: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if std::env::args().nth(1).is_some() {
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
