use std::process::ExitCode;

use pab_agent_core::{DataPaths, DataScope};
use pab_executor::{approve_claim, bootstrapped_config, run_executor, show_access};

#[tokio::main]
async fn main() -> ExitCode {
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
        return match show_access() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(%error, "show-access failed");
                eprintln!("pab-executor: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if std::env::args().nth(1).as_deref() == Some("approve-claim") {
        let Some(raw_id) = std::env::args().nth(2) else {
            eprintln!("usage: pab-executor approve-claim <claim-id>");
            return ExitCode::FAILURE;
        };
        let Ok(claim_id) = raw_id.parse() else {
            eprintln!("invalid claim ID");
            return ExitCode::FAILURE;
        };
        return match approve_claim(claim_id).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(%error, "approve-claim failed");
                eprintln!("pab-executor: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let result = match bootstrapped_config().await {
        Ok(config) => run_executor(config)
            .await
            .map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "executor failed");
            eprintln!("pab-executor: {error}");
            ExitCode::FAILURE
        }
    }
}
